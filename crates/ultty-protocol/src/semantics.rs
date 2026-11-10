//! Accessibility semantics of a region (ADR 0021, spec §10.5).
//!
//! An app says what a region *is* with `SetSemantics`; the terminal derives
//! the rest from the region's content. The role set is closed: adding a
//! role is additive, removing one is not.

use std::borrow::Cow;

use crate::{Content, RegionId, WidgetContent};

/// What a region is, for assistive technology.
///
/// The discriminant is the wire value. `Auto` asks the terminal to derive
/// the role from the region's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Role {
    /// Derived from content.
    #[default]
    Auto = 0,
    /// A container: columns, rows, stacks, cards, sections.
    Group = 1,
    /// Static text.
    Text = 2,
    /// A heading; `level` gives its depth.
    Heading = 3,
    /// An append-only transcript.
    Log = 4,
    /// Advisory status: spinners, loading states, status lines.
    Status = 5,
    /// Important, time-sensitive text such as an error.
    Alert = 6,
    /// A dialog; the `modal` flag makes it modal.
    Dialog = 7,
    /// An editable text field.
    Input = 8,
    /// A button.
    Button = 9,
    /// A checkbox; `toggled` gives its state.
    Checkbox = 10,
    /// An on/off switch; `toggled` gives its state.
    Switch = 11,
    /// A progress indicator; `value` gives its progress.
    Progress = 12,
    /// A list.
    List = 13,
    /// An item of a list; `position` gives its place.
    ListItem = 14,
    /// A row of tabs.
    TabList = 15,
    /// One tab; `position` gives its place.
    Tab = 16,
    /// A tree.
    Tree = 17,
    /// A node of a tree; `expanded` and `level` give its state and depth.
    TreeItem = 18,
    /// An image; the label is its alt text.
    Image = 19,
    /// A link.
    Link = 20,
}

impl Role {
    /// Every role, in wire order.
    pub const ALL: [Role; 21] = [
        Role::Auto,
        Role::Group,
        Role::Text,
        Role::Heading,
        Role::Log,
        Role::Status,
        Role::Alert,
        Role::Dialog,
        Role::Input,
        Role::Button,
        Role::Checkbox,
        Role::Switch,
        Role::Progress,
        Role::List,
        Role::ListItem,
        Role::TabList,
        Role::Tab,
        Role::Tree,
        Role::TreeItem,
        Role::Image,
        Role::Link,
    ];

    /// The role with this wire value, if there is one.
    pub fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.get(usize::from(value)).copied()
    }

    /// The snake_case name tools show (`ulttyctl regions`).
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Auto => "auto",
            Role::Group => "group",
            Role::Text => "text",
            Role::Heading => "heading",
            Role::Log => "log",
            Role::Status => "status",
            Role::Alert => "alert",
            Role::Dialog => "dialog",
            Role::Input => "input",
            Role::Button => "button",
            Role::Checkbox => "checkbox",
            Role::Switch => "switch",
            Role::Progress => "progress",
            Role::List => "list",
            Role::ListItem => "list_item",
            Role::TabList => "tab_list",
            Role::Tab => "tab",
            Role::Tree => "tree",
            Role::TreeItem => "tree_item",
            Role::Image => "image",
            Role::Link => "link",
        }
    }
}

/// How eagerly a screen reader announces changes to a region.
///
/// The terminal never infers liveness: a wrongly live region floods speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Live {
    /// Not announced.
    #[default]
    Off = 0,
    /// Announced when the reader is idle.
    Polite = 1,
    /// Announced at once; the platform decides whether it interrupts.
    Assertive = 2,
}

impl Live {
    /// The liveness with this wire value, if there is one.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Live::Off),
            1 => Some(Live::Polite),
            2 => Some(Live::Assertive),
            _ => None,
        }
    }

    /// The lowercase name tools show (`ulttyctl regions`).
    pub fn as_str(self) -> &'static str {
        match self {
            Live::Off => "off",
            Live::Polite => "polite",
            Live::Assertive => "assertive",
        }
    }
}

/// Checked state of a checkbox or switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Toggled {
    /// Not toggleable.
    #[default]
    Unset = 0,
    False = 1,
    True = 2,
    /// Partly checked, such as a checkbox over a mixed selection.
    Mixed = 3,
}

impl Toggled {
    /// The toggled state with this wire value, if there is one.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Toggled::Unset),
            1 => Some(Toggled::False),
            2 => Some(Toggled::True),
            3 => Some(Toggled::Mixed),
            _ => None,
        }
    }
}

/// Expanded state of a tree item or collapsible section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Expanded {
    /// Not expandable.
    #[default]
    Unset = 0,
    Collapsed = 1,
    Expanded = 2,
}

impl Expanded {
    /// The expanded state with this wire value, if there is one.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Expanded::Unset),
            1 => Some(Expanded::Collapsed),
            2 => Some(Expanded::Expanded),
            _ => None,
        }
    }
}

/// A numeric value and its range, such as a progress bar's.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SemanticValue {
    pub now: f32,
    pub min: f32,
    pub max: f32,
}

/// A region's place in a set, such as a list item's or a tab's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SetPosition {
    /// 1-based, as ARIA `aria-posinset`.
    pub index: u32,
    /// Number of items in the set.
    pub size: u32,
}

/// Everything `SetSemantics` says about a region. `Semantics::default()` is
/// what a region has before any `SetSemantics`: role `Auto`, nothing else.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Semantics {
    pub role: Role,
    /// Accessible name.
    pub label: Option<String>,
    /// Longer help text.
    pub description: Option<String>,
    pub live: Live,
    /// Work in progress, such as an open stream.
    pub busy: bool,
    /// Left out of the accessibility tree with its subtree (decoration).
    pub hidden: bool,
    pub read_only: bool,
    pub disabled: bool,
    /// Announce the whole region when part of it changes.
    pub atomic: bool,
    /// A dialog that blocks the rest of the app.
    pub modal: bool,
    pub toggled: Toggled,
    pub expanded: Expanded,
    pub value: Option<SemanticValue>,
    /// Spoken instead of the number when present.
    pub value_text: Option<String>,
    pub position: Option<SetPosition>,
    /// Heading or tree level; 0 for none.
    pub level: u8,
    /// Region whose text names this one. Not checked to exist.
    pub labelled_by: Option<RegionId>,
    /// Region whose text describes this one. Not checked to exist.
    pub described_by: Option<RegionId>,
    /// Region this one controls. Not checked to exist.
    pub controls: Option<RegionId>,
}

/// `Semantics` flag bits on the wire.
pub const SEMANTICS_FLAG_BUSY: u8 = 1;
pub const SEMANTICS_FLAG_HIDDEN: u8 = 2;
pub const SEMANTICS_FLAG_READ_ONLY: u8 = 4;
pub const SEMANTICS_FLAG_DISABLED: u8 = 8;
pub const SEMANTICS_FLAG_ATOMIC: u8 = 16;
pub const SEMANTICS_FLAG_MODAL: u8 = 32;
/// Every defined flag bit; any other bit is a decode error.
pub const SEMANTICS_FLAGS_ALL: u8 = SEMANTICS_FLAG_BUSY
    | SEMANTICS_FLAG_HIDDEN
    | SEMANTICS_FLAG_READ_ONLY
    | SEMANTICS_FLAG_DISABLED
    | SEMANTICS_FLAG_ATOMIC
    | SEMANTICS_FLAG_MODAL;

impl Semantics {
    /// The flag bits of this value, as encoded on the wire.
    pub fn flags(&self) -> u8 {
        let bit = |set: bool, flag: u8| if set { flag } else { 0 };
        bit(self.busy, SEMANTICS_FLAG_BUSY)
            | bit(self.hidden, SEMANTICS_FLAG_HIDDEN)
            | bit(self.read_only, SEMANTICS_FLAG_READ_ONLY)
            | bit(self.disabled, SEMANTICS_FLAG_DISABLED)
            | bit(self.atomic, SEMANTICS_FLAG_ATOMIC)
            | bit(self.modal, SEMANTICS_FLAG_MODAL)
    }

    /// Set the flag fields from wire bits. The caller rejects unknown bits.
    pub(crate) fn set_flags(&mut self, flags: u8) {
        self.busy = flags & SEMANTICS_FLAG_BUSY != 0;
        self.hidden = flags & SEMANTICS_FLAG_HIDDEN != 0;
        self.read_only = flags & SEMANTICS_FLAG_READ_ONLY != 0;
        self.disabled = flags & SEMANTICS_FLAG_DISABLED != 0;
        self.atomic = flags & SEMANTICS_FLAG_ATOMIC != 0;
        self.modal = flags & SEMANTICS_FLAG_MODAL != 0;
    }
}

// Derivation from content (spec §10.5), shared by the terminal and apps.

/// The most characters of a name, description or value given to
/// assistive technology; past it the text is cut and ends in "…". Names
/// are short, and a region's text can be megabytes on one line.
pub const MAX_NAME_CHARS: usize = 1024;

/// `text`, or its first [`MAX_NAME_CHARS`] characters and "…". Looks at
/// no more than that many characters and copies only when it cuts.
pub fn capped(text: &str) -> Cow<'_, str> {
    match text.char_indices().nth(MAX_NAME_CHARS) {
        None => Cow::Borrowed(text),
        Some((cut, _)) => {
            let mut short = String::with_capacity(cut + '…'.len_utf8());
            short.push_str(&text[..cut]);
            short.push('…');
            Cow::Owned(short)
        }
    }
}

/// The semantics a screen reader should see for a region whose app set
/// `explicit` and whose content is `content` (spec §10.5). `appended`
/// says whether the region has received `AppendContent`, `streaming`
/// whether a stream is open on it.
///
/// With role `Auto` (the default before any `SetSemantics`) the role comes
/// from the content. Whatever the role, fields the app left empty are
/// filled from the content where it has them; fields the app set win. A
/// region with no label is named by its text when that is one line,
/// unless it is live and has been appended to: that text is announced
/// (ADR 0021 §3), and a live node whose name changes is spoken again. An
/// input's text is its value, never its name. `live` is never inferred;
/// `busy` is set while a stream is open on the region.
///
/// The terminal uses this for what it exposes; an app can use it to send
/// `SetSemantics` only when it would change that.
pub fn derive_semantics(
    explicit: &Semantics,
    content: &Content,
    appended: bool,
    streaming: bool,
) -> Semantics {
    let mut semantics = explicit.clone();
    if semantics.role == Role::Auto {
        semantics.role = content_role(content, appended);
    }
    let announced = appended && semantics.live != Live::Off;
    fill_from_content(&mut semantics, content, !announced);
    semantics.busy |= streaming;
    semantics
}

/// The role a region's content implies.
fn content_role(content: &Content, appended: bool) -> Role {
    let text_role = if appended { Role::Log } else { Role::Text };
    match content {
        Content::None => Role::Group,
        Content::Text(_) | Content::RichText(_) => text_role,
        Content::Widget(widget) => match widget {
            WidgetContent::Markdown(_) | WidgetContent::Code(_) => text_role,
            WidgetContent::Input(_) => Role::Input,
            WidgetContent::Button(_) => Role::Button,
            WidgetContent::Progress(_) => Role::Progress,
            WidgetContent::Spinner(_) => Role::Status,
            WidgetContent::Image(_) => Role::Image,
        },
    }
}

/// Fill the fields the app left empty from what the content says; a text
/// names its region only if `named_by_text`.
fn fill_from_content(semantics: &mut Semantics, content: &Content, named_by_text: bool) {
    match content {
        Content::Widget(WidgetContent::Input(input)) => {
            if semantics.value_text.is_none() && !input.password {
                semantics.value_text = Some(input.value.clone());
            }
            if semantics.description.is_none() && !input.placeholder.is_empty() {
                semantics.description = Some(input.placeholder.clone());
            }
        }
        Content::Widget(WidgetContent::Button(button)) => {
            if semantics.label.is_none() {
                semantics.label = Some(button.label.clone());
            }
        }
        Content::Widget(WidgetContent::Progress(progress)) => {
            if progress.indeterminate {
                semantics.busy = true;
            } else if semantics.value.is_none() {
                semantics.value = Some(SemanticValue {
                    now: progress.value,
                    min: 0.0,
                    max: 1.0,
                });
            }
            if semantics.label.is_none() {
                semantics.label = progress.label.clone();
            }
        }
        Content::Widget(WidgetContent::Spinner(spinner)) => {
            semantics.busy = true;
            if semantics.label.is_none() {
                semantics.label = spinner.label.clone();
            }
        }
        _ => {
            if semantics.label.is_none() && named_by_text {
                semantics.label = one_line(content);
            }
        }
    }
}

/// The content's text when it is a single non-empty line, [`capped`]. A
/// trailing newline does not count as a second line. Allocates only in
/// that case, and no more than the capped name.
fn one_line(content: &Content) -> Option<String> {
    let text = match content {
        Content::Text(text) => &text.text,
        Content::Widget(WidgetContent::Markdown(markdown)) => &markdown.source,
        Content::Widget(WidgetContent::Code(code)) => &code.source,
        Content::RichText(rich) => {
            let last = rich.spans.len().checked_sub(1)?;
            let several_lines = rich.spans.iter().enumerate().any(|(index, span)| {
                let text = if index == last {
                    trim_line_end(&span.text)
                } else {
                    &span.text
                };
                text.contains('\n')
            });
            if several_lines {
                return None;
            }
            let mut name = String::new();
            let mut chars = 0;
            for (index, span) in rich.spans.iter().enumerate() {
                let text = if index == last {
                    trim_line_end(&span.text)
                } else {
                    &span.text
                };
                for c in text.chars() {
                    if chars == MAX_NAME_CHARS {
                        name.push('…');
                        return Some(name);
                    }
                    name.push(c);
                    chars += 1;
                }
            }
            return (!name.is_empty()).then_some(name);
        }
        _ => return None,
    };
    single_line(text).map(|line| capped(line).into_owned())
}

fn trim_line_end(text: &str) -> &str {
    let text = text.strip_suffix('\n').unwrap_or(text);
    text.strip_suffix('\r').unwrap_or(text)
}

fn single_line(text: &str) -> Option<&str> {
    let line = trim_line_end(text);
    (!line.is_empty() && !line.contains('\n')).then_some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_wire_values_follow_the_table_order() {
        for (index, role) in Role::ALL.iter().enumerate() {
            assert_eq!(*role as u8 as usize, index);
            assert_eq!(Role::from_u8(index as u8), Some(*role));
        }
        assert_eq!(Role::from_u8(21), None);
        assert_eq!(Role::Link as u8, 20);
    }

    #[test]
    fn flags_round_trip_through_bits() {
        // The defined bits are contiguous, so every value up to ALL is valid.
        for bits in 0..=SEMANTICS_FLAGS_ALL {
            let mut semantics = Semantics::default();
            semantics.set_flags(bits);
            assert_eq!(semantics.flags(), bits);
        }
    }
}
