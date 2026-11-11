//! Accessibility semantics for views (ADR 0021 §4).
//!
//! Every view says what it is with [`View::semantics`]; the native backend
//! sends that to the terminal only where it adds to what the terminal
//! derives from the region's content ([`semantics_to_send`]). [`Semantic`]
//! sets the role, name and liveness of any view, and [`Accessible`] holds
//! what a widget's `.label()`, `.description()` and `.live()` builders set.

use ratatui::layout::Rect;
use ratatui::Frame;
use ultty_protocol::{AppMessage, Content, Live, RegionId, Role, Semantics};

use super::{FocusAction, View};
use crate::event::{Event, KeyEvent};

/// The semantics worth sending for a region with `semantics` and
/// `content`: `None` when the terminal would derive the same from the
/// content alone, so plain apps cost no extra bytes.
///
/// Compared as for a region that has had no `AppendContent` and has no
/// open stream; those only change what the terminal derives for `Auto`
/// text, which is never sent.
pub(crate) fn semantics_to_send(semantics: Semantics, content: &Content) -> Option<Semantics> {
    if semantics == Semantics::default() {
        return None;
    }
    let semantics = within_codec_limits(semantics);
    let derived = ultty_protocol::derive_semantics(&Semantics::default(), content, false, false);
    let declared = ultty_protocol::derive_semantics(&semantics, content, false, false);
    (declared != derived).then_some(semantics)
}

/// Most bytes of a label or description sent: the two together with a
/// full `value_text` stay within the codec's total.
const MAX_NAME_BYTES: usize =
    (ultty_protocol::MAX_SEMANTICS_TOTAL_BYTES - ultty_protocol::MAX_SEMANTICS_STRING_LEN) / 2;

/// `semantics` with its strings cut, at a character boundary and ending
/// in "…", to what the terminal decodes: a long input value or label
/// would otherwise make it refuse the whole message.
fn within_codec_limits(mut semantics: Semantics) -> Semantics {
    for (text, limit) in [
        (&mut semantics.label, MAX_NAME_BYTES),
        (&mut semantics.description, MAX_NAME_BYTES),
        (
            &mut semantics.value_text,
            ultty_protocol::MAX_SEMANTICS_STRING_LEN,
        ),
    ] {
        if let Some(text) = text {
            cut_to_bytes(text, limit);
        }
    }
    semantics
}

fn cut_to_bytes(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let mut cut = limit - '…'.len_utf8();
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text.push('…');
}

/// Send `SetSemantics` for region `id` when `semantics` adds to what its
/// `content` gives ([`semantics_to_send`]). For widgets that create
/// regions of their own (list rows, tabs, tree rows).
pub(crate) fn send_semantics(
    send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
    id: RegionId,
    semantics: Semantics,
    content: &Content,
) -> crate::Result<()> {
    match semantics_to_send(semantics, content) {
        Some(semantics) => send_message(&AppMessage::SetSemantics { id, semantics }),
        None => Ok(()),
    }
}

/// What an app set on a widget with its `.label()`, `.description()` and
/// `.live()` builders, over the widget's own semantics.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Accessible {
    pub(crate) role: Option<Role>,
    pub(crate) label: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) live: Option<Live>,
}

impl Accessible {
    /// `semantics` with what the app set put over it.
    pub(crate) fn apply(&self, mut semantics: Semantics) -> Semantics {
        if let Some(role) = self.role {
            semantics.role = role;
        }
        if let Some(label) = &self.label {
            semantics.label = Some(label.clone());
        }
        if let Some(description) = &self.description {
            semantics.description = Some(description.clone());
        }
        if let Some(live) = self.live {
            semantics.live = live;
        }
        semantics
    }
}

/// Sets the role, accessible name, description or liveness of any view,
/// keeping the rest of what the view says about itself (a checkbox stays
/// checked, a progress bar keeps its value).
///
/// It adds no region and changes nothing on screen: the view renders as
/// it would without it, in both modes.
///
/// ```
/// use ulttui::widgets::{Column, Semantic, Text};
/// use ultty_protocol::{Live, Role};
///
/// let transcript = Semantic::new(Text::new("Bot: hello\n"))
///     .role(Role::Log)
///     .label("Messages")
///     .live(Live::Polite);
/// # let _ = Column::new().child(transcript);
/// ```
pub struct Semantic<V: View> {
    child: V,
    accessible: Accessible,
    hidden: Option<bool>,
}

impl<V: View> Semantic<V> {
    pub fn new(child: V) -> Self {
        Self {
            child,
            accessible: Accessible::default(),
            hidden: None,
        }
    }

    /// What the view is.
    pub fn role(mut self, role: Role) -> Self {
        self.accessible.role = Some(role);
        self
    }

    /// The accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.accessible.label = Some(label.into());
        self
    }

    /// Longer help text.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.accessible.description = Some(description.into());
        self
    }

    /// How text appended to the view is announced. Only text the view
    /// grows by is announced, not regions added under it.
    pub fn live(mut self, live: Live) -> Self {
        self.accessible.live = Some(live);
        self
    }

    /// Leave the view and everything in it out of the accessibility tree,
    /// as decoration.
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = Some(hidden);
        self
    }
}

impl<V: View> View for Semantic<V> {
    fn constraints(&self) -> ultty_protocol::Constraints {
        self.child.constraints()
    }

    fn style(&self) -> ultty_protocol::Style {
        self.child.style()
    }

    fn content(&self) -> Content {
        self.child.content()
    }

    fn semantics(&self) -> Semantics {
        let mut semantics = self.accessible.apply(self.child.semantics());
        if let Some(hidden) = self.hidden {
            semantics.hidden = hidden;
        }
        semantics
    }

    fn children_count(&self) -> usize {
        self.child.children_count()
    }

    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> crate::Result<()>,
        parent: RegionId,
        next_id: &mut u32,
    ) -> crate::Result<Option<RegionId>> {
        self.child
            .render_child_native(index, send_message, parent, next_id)
    }

    fn render_ansi(&self, frame: &mut Frame, area: Rect) {
        self.child.render_ansi(frame, area)
    }

    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        self.child.for_each_child(visit)
    }

    fn widget_id(&self) -> Option<&str> {
        self.child.widget_id()
    }

    fn native_key(&self) -> Option<&str> {
        self.child.native_key()
    }

    fn is_focusable(&self) -> bool {
        self.child.is_focusable()
    }

    fn on_key(&mut self, key: &KeyEvent) -> Option<FocusAction> {
        self.child.on_key(key)
    }

    fn on_event(&mut self, event: &Event) -> bool {
        self.child.on_event(event)
    }

    fn scroll_offset(&self) -> Option<(f32, f32)> {
        self.child.scroll_offset()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{render_view_messages, Button, Checkbox, Column, Text};
    use ultty_protocol::{ButtonWidget, TextContent, Toggled};

    fn sent_semantics(view: &dyn View) -> Vec<(RegionId, Semantics)> {
        render_view_messages(view)
            .unwrap()
            .into_iter()
            .filter_map(|message| match message {
                AppMessage::SetSemantics { id, semantics } => Some((id, semantics)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn nothing_is_sent_when_content_already_says_it() {
        let button = Content::Widget(ultty_protocol::WidgetContent::Button(ButtonWidget::new(
            "Run",
        )));
        let same = Semantics {
            role: Role::Button,
            label: Some("Run".to_string()),
            ..Semantics::default()
        };
        assert_eq!(semantics_to_send(same, &button), None);
        assert_eq!(semantics_to_send(Semantics::default(), &button), None);

        let renamed = Semantics {
            role: Role::Button,
            label: Some("Run all tests".to_string()),
            ..Semantics::default()
        };
        assert_eq!(
            semantics_to_send(renamed.clone(), &button),
            Some(renamed),
            "a name the content does not give is sent"
        );

        let text = Content::Text(TextContent::new("hello"));
        let plain = Semantics {
            role: Role::Text,
            ..Semantics::default()
        };
        assert_eq!(semantics_to_send(plain, &text), None);
        let live = Semantics {
            live: Live::Polite,
            ..Semantics::default()
        };
        assert_eq!(semantics_to_send(live.clone(), &text), Some(live));
    }

    #[test]
    fn wrapper_sets_role_label_and_live_on_the_child_region_itself() {
        let view = Semantic::new(Text::new("Bot: hi"))
            .role(Role::Log)
            .label("Messages")
            .description("The conversation")
            .live(Live::Polite);
        let messages = render_view_messages(&view).unwrap();
        let regions = messages
            .iter()
            .filter(|message| matches!(message, AppMessage::CreateRegion { .. }))
            .count();
        assert_eq!(regions, 1, "the wrapper adds no region");
        assert_eq!(
            sent_semantics(&view),
            vec![(
                RegionId::new(1),
                Semantics {
                    role: Role::Log,
                    label: Some("Messages".to_string()),
                    description: Some("The conversation".to_string()),
                    live: Live::Polite,
                    ..Semantics::default()
                }
            )]
        );
        assert_eq!(view.content(), View::content(&Text::new("Bot: hi")));
    }

    #[test]
    fn wrapper_keeps_the_child_state_it_does_not_set() {
        let view = Semantic::new(Checkbox::new("Accept").checked(true)).label("Accept terms");
        let semantics = view.semantics();
        assert_eq!(semantics.role, Role::Checkbox);
        assert_eq!(semantics.toggled, Toggled::True);
        assert_eq!(semantics.label.as_deref(), Some("Accept terms"));
    }

    #[test]
    fn wrapper_on_a_container_names_the_container_and_forwards_children() {
        let view = Semantic::new(
            Column::new()
                .child(Button::new("OK").id("ok"))
                .child(Text::new("x")),
        )
        .role(Role::Group)
        .label("Actions");
        assert_eq!(view.children_count(), 2);
        let mut visited = 0;
        view.for_each_child(&mut |_| visited += 1);
        assert_eq!(visited, 2);
        assert_eq!(
            sent_semantics(&view),
            vec![(
                RegionId::new(1),
                Semantics {
                    role: Role::Group,
                    label: Some("Actions".to_string()),
                    ..Semantics::default()
                }
            )]
        );
    }

    /// Semantics sent for each region `view` renders, by region order.
    fn per_region(view: &dyn View) -> Vec<Option<Semantics>> {
        let messages = render_view_messages(view).unwrap();
        let mut regions: Vec<(RegionId, Option<Semantics>)> = Vec::new();
        for message in messages {
            match message {
                AppMessage::CreateRegion { id, .. } => regions.push((id, None)),
                AppMessage::SetSemantics { id, semantics } => {
                    let slot = regions.iter_mut().find(|(region, _)| *region == id);
                    slot.expect("semantics after create").1 = Some(semantics);
                }
                _ => {}
            }
        }
        regions
            .into_iter()
            .map(|(_, semantics)| semantics)
            .collect()
    }

    #[test]
    fn widget_defaults() {
        use crate::widgets::{
            Badge, Divider, ErrorState, Image, ProgressBar, Section, Select, Spacer, Spinner,
            TextInput, Title,
        };
        use ultty_protocol::{Expanded, SemanticValue, SetPosition};

        let progress = ProgressBar::new(40.0).label("Upload").semantics();
        assert_eq!(progress.role, Role::Progress);
        assert_eq!(
            progress.value,
            Some(SemanticValue {
                now: 0.4,
                min: 0.0,
                max: 1.0
            })
        );
        assert_eq!(
            per_region(&ProgressBar::new(40.0)),
            vec![None],
            "as derived"
        );

        let spinner = Spinner::new().label("Thinking").semantics();
        assert_eq!((spinner.role, spinner.busy), (Role::Status, true));
        assert_eq!(spinner.label.as_deref(), Some("Thinking"));

        let image = Image::new(vec![0; 4], 1, 1).label("A red dot").semantics();
        assert_eq!(image.role, Role::Image);
        assert_eq!(image.label.as_deref(), Some("A red dot"));

        assert!(Divider::horizontal().semantics().hidden);
        assert!(Spacer.semantics().hidden);
        assert_eq!(per_region(&Divider::horizontal()).len(), 1);
        assert!(
            per_region(&Divider::horizontal())[0]
                .as_ref()
                .unwrap()
                .hidden
        );

        let title = Title::new("Chat").semantics();
        assert_eq!((title.role, title.level), (Role::Heading, 1));
        assert_eq!(Title::new("Files").level(2).semantics().level, 2);
        let section = per_region(&Section::new().title("Files").child(Text::new("a")));
        assert_eq!(section[0].as_ref().unwrap().label.as_deref(), Some("Files"));
        assert_eq!(
            section[1].as_ref().unwrap().level,
            2,
            "its title is a level 2 heading"
        );

        let badge = Badge::success("passing").semantics();
        assert_eq!(badge.role, Role::Status);
        assert_eq!(badge.label.as_deref(), Some("passing"));

        let error = ErrorState::new("disk full").hint("free space").semantics();
        assert_eq!(error.role, Role::Alert);
        assert_eq!(error.label.as_deref(), Some("Error: disk full"));
        assert_eq!(error.description.as_deref(), Some("free space"));

        let input = TextInput::new()
            .value("hi")
            .placeholder("Say something")
            .label("Message")
            .semantics();
        assert_eq!(input.role, Role::Input);
        assert_eq!(input.label.as_deref(), Some("Message"));
        assert_eq!(input.value_text.as_deref(), Some("hi"));
        // The description carries the vim mode besides the placeholder.
        assert_eq!(
            input.description.as_deref(),
            Some("Say something. vim normal mode")
        );
        let password = TextInput::new().value("hunter2").password();
        assert_eq!(password.semantics().value_text, None);
        let ultty_protocol::Content::Widget(ultty_protocol::WidgetContent::Input(field)) =
            password.content()
        else {
            panic!("an input");
        };
        assert!(field.password);

        let select = Select::new(vec!["a".into(), "b".into(), "c".into()])
            .selected(1)
            .label("Letter")
            .semantics();
        assert_eq!(select.role, Role::List);
        assert_eq!(select.expanded, Expanded::Collapsed);
        assert_eq!(select.value_text.as_deref(), Some("b"));
        assert_eq!(select.position, Some(SetPosition { index: 2, size: 3 }));
        assert_eq!(select.label.as_deref(), Some("Letter"));
    }

    #[test]
    fn widgets_that_draw_their_own_rows_describe_each_row() {
        use crate::widgets::{Collapsible, List, Modal, Tabs, Tree, TreeNode};
        use ultty_protocol::{Expanded, SetPosition};

        let list = per_region(&List::new().items(vec!["one", "two"]).label("Numbers"));
        let list_semantics = list[0].as_ref().unwrap();
        assert_eq!(list_semantics.role, Role::List);
        assert_eq!(list_semantics.label.as_deref(), Some("Numbers"));
        let second = list[2].as_ref().unwrap();
        assert_eq!(second.role, Role::ListItem);
        assert_eq!(second.position, Some(SetPosition { index: 2, size: 2 }));

        let tabs = per_region(&Tabs::new().labels(vec!["Log", "Diff"]).active(1));
        assert_eq!(tabs[0].as_ref().unwrap().role, Role::TabList);
        let diff = tabs[2].as_ref().unwrap();
        assert_eq!(diff.role, Role::Tab);
        assert_eq!(diff.label.as_deref(), Some("Diff"), "without the padding");
        assert_eq!(diff.position, Some(SetPosition { index: 2, size: 2 }));

        let tree = Tree::new(
            TreeNode::with_children("src", vec![TreeNode::new("main.rs")]).expanded(true),
        )
        .label("Files");
        let regions = per_region(&tree);
        assert_eq!(regions[0].as_ref().unwrap().role, Role::Tree);
        let rows: Vec<&Semantics> = regions
            .iter()
            .flatten()
            .filter(|semantics| semantics.role == Role::TreeItem)
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label.as_deref(), Some("src"));
        assert_eq!((rows[0].level, rows[0].expanded), (1, Expanded::Expanded));
        assert_eq!((rows[1].level, rows[1].expanded), (2, Expanded::Unset));
        let hidden = regions.iter().flatten().filter(|s| s.hidden).count();
        assert_eq!(
            hidden,
            regions.len() - 3,
            "every text drawn in a row is hidden"
        );

        let collapsible = Collapsible::new(Text::new("More"), Text::new("body"));
        assert_eq!(collapsible.semantics().role, Role::TreeItem);
        assert_eq!(collapsible.semantics().expanded, Expanded::Collapsed);

        let modal = Modal::new()
            .title("Delete?")
            .body("Really?")
            .buttons(vec!["Yes", "No"]);
        let regions = per_region(&modal);
        let dialog = regions[0].as_ref().unwrap();
        assert_eq!(dialog.role, Role::Dialog);
        assert!(dialog.modal);
        assert_eq!(dialog.label.as_deref(), Some("Delete?"));
        let buttons: Vec<_> = regions
            .iter()
            .flatten()
            .filter(|s| s.role == Role::Button)
            .map(|s| s.label.clone().unwrap())
            .collect();
        assert_eq!(buttons, vec!["Yes", "No"]);
        assert!(Modal::new().visible(false).semantics().hidden);
    }

    #[test]
    fn long_strings_are_cut_to_what_the_terminal_decodes() {
        use crate::widgets::TextInput;
        use ultty_protocol::{Decode, Encode};

        let huge = "é".repeat(10_000);
        let input = TextInput::new()
            .value(huge.clone())
            .label(huge.clone())
            .description(huge);
        let mut checked = 0;
        for message in render_view_messages(&input).unwrap() {
            let bytes = message.encode_to_bytes();
            let decoded = AppMessage::decode(&mut &bytes[..]).expect("decodes");
            if let AppMessage::SetSemantics { semantics, .. } = decoded {
                let value = semantics.value_text.unwrap();
                assert!(value.len() <= ultty_protocol::MAX_SEMANTICS_STRING_LEN);
                assert!(value.ends_with('…') && value.starts_with('é'));
                assert!(semantics.label.unwrap().len() <= MAX_NAME_BYTES);
                checked += 1;
            }
        }
        assert_eq!(checked, 1);
    }

    #[test]
    fn hidden_leaves_a_view_out() {
        let view = Semantic::new(Text::new("~~~~")).hidden(true);
        assert!(view.semantics().hidden);
        assert_eq!(sent_semantics(&view).len(), 1);
    }
}
