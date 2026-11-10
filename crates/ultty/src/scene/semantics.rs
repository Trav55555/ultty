//! A region's effective accessibility semantics: what the app set with
//! `SetSemantics`, completed from the region's content (spec §10.5).

use ultty_protocol::Semantics;

pub use ultty_protocol::{capped, MAX_NAME_CHARS};

use super::Region;

/// The semantics a screen reader should see for `region`: what the app
/// set, completed from the region's content
/// ([`ultty_protocol::derive_semantics`]).
pub fn effective_semantics(region: &Region) -> Semantics {
    ultty_protocol::derive_semantics(
        region.semantics(),
        &region.content,
        region.appended,
        region.streaming,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultty_protocol::{
        ButtonWidget, Content, ImageScale, ImageWidget, InputWidget, Live, ProgressWidget,
        RegionId, RichText, Role, SemanticValue, Span, SpinnerWidget, Style, TextContent,
        WidgetContent,
    };

    fn region(content: Content) -> Region {
        let mut region = Region::new(RegionId::new(1));
        region.content = content;
        region
    }

    fn text(text: &str) -> Content {
        Content::Text(TextContent::new(text))
    }

    fn widget(widget: WidgetContent) -> Content {
        Content::Widget(widget)
    }

    #[test]
    fn empty_region_is_an_unlabelled_group() {
        let semantics = effective_semantics(&region(Content::None));
        assert_eq!(semantics.role, Role::Group);
        assert_eq!(semantics.label, None);
        assert_eq!(semantics.live, Live::Off);
    }

    #[test]
    fn one_line_text_is_labelled_text() {
        let semantics = effective_semantics(&region(text("Build\n")));
        assert_eq!(semantics.role, Role::Text);
        assert_eq!(semantics.label.as_deref(), Some("Build"));
    }

    #[test]
    fn several_lines_of_text_have_no_label() {
        let semantics = effective_semantics(&region(text("compiling\nfinished")));
        assert_eq!(semantics.role, Role::Text);
        assert_eq!(semantics.label, None);
        assert_eq!(effective_semantics(&region(text(""))).label, None);
    }

    #[test]
    fn every_text_kind_becomes_a_log_once_appended_to() {
        let rich = Content::RichText(RichText {
            spans: vec![Span {
                text: "ok".to_string(),
                style: Style::new(),
            }],
        });
        let markdown = widget(WidgetContent::Markdown(ultty_protocol::MarkdownWidget {
            source: "# hi".to_string(),
        }));
        let code = widget(WidgetContent::Code(ultty_protocol::CodeWidget {
            source: "fn main() {}".to_string(),
            language: "rust".to_string(),
            line_numbers: false,
        }));
        for content in [text("one"), rich, markdown, code] {
            let mut region = region(content);
            assert_eq!(effective_semantics(&region).role, Role::Text);
            region.appended = true;
            let semantics = effective_semantics(&region);
            assert_eq!(semantics.role, Role::Log);
            assert_eq!(semantics.live, Live::Off, "liveness is never inferred");
        }
    }

    #[test]
    fn an_open_stream_makes_any_region_busy() {
        let mut region = region(text("partial"));
        assert!(!effective_semantics(&region).busy);
        region.streaming = true;
        assert!(effective_semantics(&region).busy);
        // Not live and never appended: still named by its one line.
        assert_eq!(
            effective_semantics(&region).label.as_deref(),
            Some("partial")
        );
    }

    #[test]
    fn a_live_region_set_whole_is_still_named_by_its_text() {
        let mut status = region(text("Building"));
        status.semantics_mut().live = Live::Polite;
        assert_eq!(
            effective_semantics(&status).label.as_deref(),
            Some("Building")
        );
    }

    #[test]
    fn input_exposes_value_and_placeholder_but_is_not_named_by_them() {
        let semantics = effective_semantics(&region(widget(WidgetContent::Input(InputWidget {
            value: "cargo test".to_string(),
            placeholder: "Command".to_string(),
            ..Default::default()
        }))));
        assert_eq!(semantics.role, Role::Input);
        assert_eq!(semantics.value_text.as_deref(), Some("cargo test"));
        assert_eq!(semantics.description.as_deref(), Some("Command"));
        assert_eq!(semantics.label, None);
    }

    #[test]
    fn password_input_does_not_expose_its_value() {
        let semantics = effective_semantics(&region(widget(WidgetContent::Input(InputWidget {
            value: "hunter2".to_string(),
            password: true,
            ..Default::default()
        }))));
        assert_eq!(semantics.role, Role::Input);
        assert_eq!(semantics.value_text, None);
    }

    #[test]
    fn button_is_named_by_its_label() {
        let semantics = effective_semantics(&region(widget(WidgetContent::Button(
            ButtonWidget::new("Run"),
        ))));
        assert_eq!(semantics.role, Role::Button);
        assert_eq!(semantics.label.as_deref(), Some("Run"));
    }

    #[test]
    fn progress_reports_its_value_or_busy_when_indeterminate() {
        let progress = |value, indeterminate| {
            effective_semantics(&region(widget(WidgetContent::Progress(ProgressWidget {
                value,
                label: Some("Download".to_string()),
                animated: false,
                indeterminate,
            }))))
        };
        let determinate = progress(0.25, false);
        assert_eq!(determinate.role, Role::Progress);
        assert_eq!(
            determinate.value,
            Some(SemanticValue {
                now: 0.25,
                min: 0.0,
                max: 1.0
            })
        );
        assert!(!determinate.busy);
        assert_eq!(determinate.label.as_deref(), Some("Download"));

        let indeterminate = progress(0.0, true);
        assert_eq!(indeterminate.role, Role::Progress);
        assert_eq!(indeterminate.value, None);
        assert!(indeterminate.busy);
    }

    #[test]
    fn spinner_is_a_busy_status() {
        let semantics =
            effective_semantics(&region(widget(WidgetContent::Spinner(SpinnerWidget {
                label: Some("Thinking".to_string()),
                ..Default::default()
            }))));
        assert_eq!(semantics.role, Role::Status);
        assert!(semantics.busy);
        assert_eq!(semantics.label.as_deref(), Some("Thinking"));
    }

    #[test]
    fn image_is_unlabelled() {
        let semantics = effective_semantics(&region(widget(WidgetContent::Image(ImageWidget {
            width: 1,
            height: 1,
            data: vec![0; 4],
            scale: ImageScale::Fit,
        }))));
        assert_eq!(semantics.role, Role::Image);
        assert_eq!(semantics.label, None);
    }

    #[test]
    fn explicit_semantics_override_and_auto_keeps_what_the_app_set() {
        let mut log = region(text("hello"));
        log.appended = true;
        log.set_semantics(Semantics {
            role: Role::Status,
            live: Live::Polite,
            ..Semantics::default()
        });
        let semantics = effective_semantics(&log);
        assert_eq!(semantics.role, Role::Status);
        assert_eq!(semantics.live, Live::Polite);
        assert_eq!(
            semantics.label, None,
            "appended text is announced, not its name (a renamed live node is spoken again)"
        );

        let mut button = region(widget(WidgetContent::Button(ButtonWidget::new("Run"))));
        button.semantics_mut().label = Some("Run all tests".to_string());
        button.semantics_mut().disabled = true;
        let semantics = effective_semantics(&button);
        assert_eq!(semantics.role, Role::Button);
        assert_eq!(semantics.label.as_deref(), Some("Run all tests"));
        assert!(semantics.disabled);
    }

    #[test]
    fn explicit_role_still_gets_state_from_content() {
        let mut button = region(widget(WidgetContent::Button(ButtonWidget::new("Run"))));
        button.semantics_mut().role = Role::Button;
        assert_eq!(effective_semantics(&button).label.as_deref(), Some("Run"));

        let mut bar = region(widget(WidgetContent::Progress(ProgressWidget {
            value: 0.5,
            label: Some("Upload".to_string()),
            animated: false,
            indeterminate: false,
        })));
        bar.semantics_mut().role = Role::Progress;
        let semantics = effective_semantics(&bar);
        assert_eq!(semantics.value.map(|value| value.now), Some(0.5));
        assert_eq!(semantics.label.as_deref(), Some("Upload"));

        let mut spinner = region(widget(WidgetContent::Spinner(SpinnerWidget::default())));
        spinner.semantics_mut().role = Role::Alert;
        let semantics = effective_semantics(&spinner);
        assert_eq!(semantics.role, Role::Alert);
        assert!(semantics.busy);

        let mut field = region(widget(WidgetContent::Input(InputWidget {
            value: "secret".to_string(),
            placeholder: "Token".to_string(),
            password: true,
            ..Default::default()
        })));
        field.semantics_mut().role = Role::Input;
        field.semantics_mut().description = Some("API token".to_string());
        let semantics = effective_semantics(&field);
        assert_eq!(semantics.value_text, None, "never a password");
        assert_eq!(semantics.description.as_deref(), Some("API token"));
        assert_eq!(semantics.label, None, "an input is not named by its value");

        let mut status = region(text("Saved"));
        status.semantics_mut().role = Role::Status;
        status.semantics_mut().value = Some(SemanticValue {
            now: 3.0,
            min: 0.0,
            max: 9.0,
        });
        let semantics = effective_semantics(&status);
        assert_eq!(semantics.label.as_deref(), Some("Saved"));
        assert_eq!(semantics.value.map(|value| value.now), Some(3.0));
    }

    #[test]
    fn a_long_line_names_its_region_only_up_to_the_cap() {
        let line = "é".repeat(2 << 20);
        let name = |content| effective_semantics(&region(content)).label.expect("a name");
        let expected = format!("{}…", "é".repeat(MAX_NAME_CHARS));
        assert_eq!(name(text(&line)), expected);
        let rich = Content::RichText(RichText {
            spans: vec![
                Span {
                    text: line[..2 << 20].to_string(),
                    style: Style::new(),
                },
                Span {
                    text: format!("{}\n", &line[2 << 20..]),
                    style: Style::new(),
                },
            ],
        });
        assert_eq!(name(rich), expected);
        // At the cap exactly, nothing is cut.
        let exact = "x".repeat(MAX_NAME_CHARS);
        assert_eq!(name(text(&exact)), exact);
    }

    #[test]
    fn rich_text_is_one_line_only_without_inner_newlines() {
        let rich = |parts: &[&str]| {
            Content::RichText(RichText {
                spans: parts
                    .iter()
                    .map(|text| Span {
                        text: text.to_string(),
                        style: Style::new(),
                    })
                    .collect(),
            })
        };
        let label = |parts: &[&str]| effective_semantics(&region(rich(parts))).label;
        assert_eq!(label(&["Build ", "ok\n"]).as_deref(), Some("Build ok"));
        assert_eq!(label(&["Build\n", "ok"]), None);
        assert_eq!(label(&[]), None);
    }
}
