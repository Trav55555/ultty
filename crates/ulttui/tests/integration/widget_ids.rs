//! Every widget that renders a region takes `.id()`, and that id is what
//! user overrides (ADR 0023) and focus key on.
//!
//! Each case puts one widget with id `target` in a column beside an
//! id-less sibling and hides it by id. The widget's region must go
//! `Display::None` in native mode and its ANSI cells must disappear; the
//! same tree without the override must not hide anything. A widget that
//! ignored its id would be untouched by the override.

use std::rc::Rc;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use ulttui::overrides::{with_overrides, Overrides};
use ulttui::widgets::{
    render_view_ansi, render_view_messages, Accent, Badge, Body, Card, Column, Divider, DockLayout,
    EmptyState, ErrorState, Image, List, LoadingState, MarkdownView, Modal, Muted, ProgressBar,
    RichText, Row, ScrollView, Section, Spinner, Stack, Table, Tabs, Text, Title, View,
};
use ultty_protocol::{AppMessage, Display};

type Make = fn() -> Box<dyn View>;

/// A boxed view as a child (the crate has no `View for Box<dyn View>`).
struct Dyn(Box<dyn View>);

impl View for Dyn {
    fn constraints(&self) -> ultty_protocol::Constraints {
        self.0.constraints()
    }
    fn style(&self) -> ultty_protocol::Style {
        self.0.style()
    }
    fn content(&self) -> ultty_protocol::Content {
        self.0.content()
    }
    fn semantics(&self) -> ultty_protocol::Semantics {
        self.0.semantics()
    }
    fn children_count(&self) -> usize {
        self.0.children_count()
    }
    fn render_child_native(
        &self,
        index: usize,
        send_message: &mut dyn FnMut(&AppMessage) -> ulttui::Result<()>,
        parent: ultty_protocol::RegionId,
        next_id: &mut u32,
    ) -> ulttui::Result<Option<ultty_protocol::RegionId>> {
        self.0
            .render_child_native(index, send_message, parent, next_id)
    }
    fn render_ansi(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        self.0.render_ansi(frame, area)
    }
    fn for_each_child(&self, visit: &mut dyn FnMut(&dyn View)) {
        self.0.for_each_child(visit)
    }
    fn widget_id(&self) -> Option<&str> {
        self.0.widget_id()
    }
}

fn cases() -> Vec<(&'static str, Make)> {
    vec![
        ("Column", || {
            Box::new(Column::new().child(Text::new("c")).id("target"))
        }),
        ("Row", || {
            Box::new(Row::new().child(Text::new("r")).id("target"))
        }),
        ("Stack", || {
            Box::new(Stack::new().child(Text::new("s")).id("target"))
        }),
        ("ScrollView", || {
            Box::new(ScrollView::new().child(Text::new("sv")).id("target"))
        }),
        ("List", || {
            Box::new(
                List::<String>::new()
                    .with_appended("item".into())
                    .id("target"),
            )
        }),
        ("Table", || Box::new(Table::new().id("target"))),
        ("ProgressBar", || {
            Box::new(ProgressBar::new(40.0).id("target"))
        }),
        ("Spinner", || Box::new(Spinner::new().id("target"))),
        ("MarkdownView", || {
            Box::new(MarkdownView::new("# hi").id("target"))
        }),
        ("Tabs", || Box::new(Tabs::new().id("target"))),
        ("Modal", || Box::new(Modal::new().id("target"))),
        ("DockLayout", || Box::new(DockLayout::new().id("target"))),
        ("Badge", || Box::new(Badge::new("b").id("target"))),
        ("Divider", || Box::new(Divider::horizontal().id("target"))),
        ("Image", || {
            Box::new(Image::new(vec![0; 4], 1, 1).id("target"))
        }),
        ("Title", || Box::new(Title::new("t").id("target"))),
        ("Body", || Box::new(Body::new("b").id("target"))),
        ("Muted", || Box::new(Muted::new("m").id("target"))),
        ("Accent", || Box::new(Accent::new("a").id("target"))),
        ("Section", || Box::new(Section::new().id("target"))),
        ("Card", || Box::new(Card::new().id("target"))),
        ("Text", || Box::new(Text::new("text").id("target"))),
        ("RichText", || {
            Box::new(
                RichText::new()
                    .span("rich", Default::default())
                    .id("target"),
            )
        }),
        ("EmptyState", || {
            Box::new(EmptyState::new("none").id("target"))
        }),
        ("LoadingState", || {
            Box::new(LoadingState::new("wait").id("target"))
        }),
        ("ErrorState", || {
            Box::new(ErrorState::new("bad").id("target"))
        }),
    ]
}

fn hidden_regions(view: &dyn View, source: &str) -> usize {
    let overrides = Rc::new(Overrides::parse(source).unwrap());
    with_overrides(overrides, || render_view_messages(view).unwrap())
        .iter()
        .filter(|m| {
            matches!(
                m,
                AppMessage::CreateRegion { constraints, .. } if constraints.display == Display::None
            )
        })
        .count()
}

fn ansi_text(view: &dyn View, source: &str) -> String {
    let overrides = Rc::new(Overrides::parse(source).unwrap());
    let mut terminal = Terminal::new(TestBackend::new(30, 12)).unwrap();
    with_overrides(overrides, || {
        terminal
            .draw(|frame| render_view_ansi(view, frame, frame.area()))
            .unwrap()
    });
    format!("{:?}", terminal.backend().buffer())
}

#[test]
fn an_override_keyed_by_the_widgets_id_hides_its_region() {
    for (name, make) in cases() {
        let view = Column::new().child(Text::new("sibling")).child(Dyn(make()));
        let before = hidden_regions(&view, "");
        let after = hidden_regions(&view, "[widget.target]\nhidden = true");
        assert_eq!(after, before + 1, "{name}: id did not reach the override");
        let unrelated = hidden_regions(&view, "[widget.other]\nhidden = true");
        assert_eq!(
            unrelated, before,
            "{name}: an override for another id applied"
        );
    }
}

#[test]
fn a_hidden_widget_draws_nothing_in_ansi_mode() {
    // Image, Modal and DockLayout/Tabs/Table/Section/Card draw nothing or
    // only decoration when empty; they are covered by the native check above.
    for (name, make) in cases() {
        if [
            "Image",
            "Modal",
            "DockLayout",
            "Tabs",
            "Table",
            "Section",
            "Card",
        ]
        .contains(&name)
        {
            continue;
        }
        let view = Column::new().child(Dyn(make()));
        let shown = ansi_text(&view, "");
        let hidden = ansi_text(&view, "[widget.target]\nhidden = true");
        assert_ne!(shown, hidden, "{name}: still drawn when hidden");
    }
}
