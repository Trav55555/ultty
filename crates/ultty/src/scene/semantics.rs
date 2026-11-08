//! A region's effective accessibility semantics: what the app set with
//! `SetSemantics`, completed from the region's content (spec §10.5).

use ultty_protocol::{Content, Role, SemanticValue, Semantics, WidgetContent};

use super::Region;

/// The semantics a screen reader should see for `region`.
///
/// With role `Auto` (the default before any `SetSemantics`) the role and
/// its state come from the content. Fields the app set are kept; this only
/// fills what it left empty. A region with no label is named by its text
/// when that is one line, except an input, whose text is its value. `live`
/// is never inferred.
pub fn effective_semantics(region: &Region) -> Semantics {
    let mut semantics = region.semantics.clone();
    if semantics.role == Role::Auto {
        derive_from_content(&mut semantics, &region.content, region.appended);
    }
    if semantics.label.is_none() && semantics.role != Role::Input {
        semantics.label = one_line(&region.content);
    }
    semantics
}

fn derive_from_content(semantics: &mut Semantics, content: &Content, appended: bool) {
    let text_role = if appended { Role::Log } else { Role::Text };
    semantics.role = match content {
        Content::None => Role::Group,
        Content::Text(_) | Content::RichText(_) => text_role,
        Content::Widget(widget) => match widget {
            WidgetContent::Markdown(_) | WidgetContent::Code(_) => text_role,
            WidgetContent::Input(input) => {
                if semantics.value_text.is_none() && !input.password {
                    semantics.value_text = Some(input.value.clone());
                }
                if semantics.description.is_none() && !input.placeholder.is_empty() {
                    semantics.description = Some(input.placeholder.clone());
                }
                Role::Input
            }
            WidgetContent::Button(button) => {
                if semantics.label.is_none() {
                    semantics.label = Some(button.label.clone());
                }
                Role::Button
            }
            WidgetContent::Progress(progress) => {
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
                Role::Progress
            }
            WidgetContent::Spinner(spinner) => {
                semantics.busy = true;
                if semantics.label.is_none() {
                    semantics.label = spinner.label.clone();
                }
                Role::Status
            }
            WidgetContent::Image(_) => Role::Image,
        },
    };
}

/// The content's text when it is a single non-empty line. A trailing
/// newline does not count as a second line.
fn one_line(content: &Content) -> Option<String> {
    let text = match content {
        Content::Text(text) => text.text.clone(),
        Content::RichText(rich) => rich.spans.iter().map(|span| span.text.as_str()).collect(),
        Content::Widget(WidgetContent::Markdown(markdown)) => markdown.source.clone(),
        Content::Widget(WidgetContent::Code(code)) => code.source.clone(),
        _ => return None,
    };
    let line = text.strip_suffix('\n').unwrap_or(&text);
    let line = line.strip_suffix('\r').unwrap_or(line);
    (!line.is_empty() && !line.contains('\n')).then(|| line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultty_protocol::{
        ButtonWidget, ImageScale, ImageWidget, InputWidget, Live, ProgressWidget, RegionId,
        RichText, Span, SpinnerWidget, Style, TextContent,
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
        log.semantics = Semantics {
            role: Role::Status,
            live: Live::Polite,
            ..Semantics::default()
        };
        let semantics = effective_semantics(&log);
        assert_eq!(semantics.role, Role::Status);
        assert_eq!(semantics.live, Live::Polite);
        assert_eq!(semantics.label.as_deref(), Some("hello"), "text names it");

        let mut button = region(widget(WidgetContent::Button(ButtonWidget::new("Run"))));
        button.semantics.label = Some("Run all tests".to_string());
        button.semantics.disabled = true;
        let semantics = effective_semantics(&button);
        assert_eq!(semantics.role, Role::Button);
        assert_eq!(semantics.label.as_deref(), Some("Run all tests"));
        assert!(semantics.disabled);
    }
}
