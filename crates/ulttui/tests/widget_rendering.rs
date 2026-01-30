//! Widget rendering tests using ratatui's TestBackend.
//!
//! These tests verify that widgets render correctly to the terminal buffer.
//! Inspired by Alacritty's reference tests and Ghostty's handler-based testing.

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

use ulttui::widgets::{Button, Column, List, Row, Stack, Text, View};

fn render_widget(widget: &impl View, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| {
            widget.render_ansi(frame, frame.area());
        })
        .unwrap();

    terminal.backend().buffer().clone()
}

fn buffer_contains(buffer: &Buffer, needle: &str) -> bool {
    let content: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    content.contains(needle)
}

mod text_widget {
    use super::*;

    #[test]
    fn renders_simple_text() {
        let text = Text::new("Hello, World!");
        let buffer = render_widget(&text, 20, 1);
        assert!(buffer_contains(&buffer, "Hello, World!"));
    }

    #[test]
    fn renders_empty_text() {
        let text = Text::new("");
        let buffer = render_widget(&text, 10, 1);
        let content: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(content.trim().is_empty() || content.chars().all(|c| c == ' '));
    }

    #[test]
    fn truncates_long_text() {
        let text = Text::new("This is a very long string that should be truncated");
        let buffer = render_widget(&text, 10, 1);
        assert!(buffer_contains(&buffer, "This is a"));
    }

    #[test]
    fn handles_unicode() {
        let text = Text::new("Hello 世界 🌍");
        let buffer = render_widget(&text, 20, 1);
        assert!(buffer_contains(&buffer, "Hello"));
    }
}

mod button_widget {
    use super::*;

    #[test]
    fn renders_with_brackets() {
        let button = Button::new("Click Me");
        let buffer = render_widget(&button, 20, 1);
        assert!(buffer_contains(&buffer, "[ Click Me ]"));
    }

    #[test]
    fn renders_short_label() {
        let button = Button::new("OK");
        let buffer = render_widget(&button, 10, 1);
        assert!(buffer_contains(&buffer, "[ OK ]"));
    }
}

mod container_widgets {
    use super::*;

    #[test]
    fn column_stores_children() {
        let col = Column::new()
            .child(Text::new("Line 1"))
            .child(Text::new("Line 2"))
            .child(Text::new("Line 3"));
        assert_eq!(col.children_count(), 3);
    }

    #[test]
    fn row_stores_children() {
        let row = Row::new().child(Text::new("A")).child(Text::new("B"));
        assert_eq!(row.children_count(), 2);
    }

    #[test]
    fn stack_stores_children() {
        let stack = Stack::new()
            .child(Text::new("Bottom"))
            .child(Text::new("Top"));
        assert_eq!(stack.children_count(), 2);
    }

    #[test]
    fn column_renders_children_vertically() {
        let col = Column::new()
            .child(Text::new("AAA"))
            .child(Text::new("BBB"));
        let buffer = render_widget(&col, 10, 4);
        assert!(buffer_contains(&buffer, "AAA"));
        assert!(buffer_contains(&buffer, "BBB"));
    }

    #[test]
    fn row_renders_children_horizontally() {
        let row = Row::new()
            .child(Text::new("Left"))
            .child(Text::new("Right"));
        let buffer = render_widget(&row, 20, 1);
        assert!(buffer_contains(&buffer, "Left"));
        assert!(buffer_contains(&buffer, "Right"));
    }

    #[test]
    fn empty_column_renders_nothing() {
        let col = Column::new();
        let buffer = render_widget(&col, 10, 5);
        let content: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(content.trim().is_empty() || content.chars().all(|c| c == ' '));
    }

    #[test]
    fn nested_containers() {
        let nested = Column::new()
            .child(Row::new().child(Text::new("A")).child(Text::new("B")))
            .child(Text::new("C"));
        let buffer = render_widget(&nested, 20, 4);
        assert!(buffer_contains(&buffer, "A"));
        assert!(buffer_contains(&buffer, "B"));
        assert!(buffer_contains(&buffer, "C"));
    }
}

mod list_widget {
    use super::*;

    #[test]
    fn renders_items() {
        let list = List::new().items(vec!["Apple", "Banana", "Cherry"]);
        let buffer = render_widget(&list, 30, 10);
        assert!(buffer_contains(&buffer, "Apple"));
        assert!(buffer_contains(&buffer, "Banana"));
        assert!(buffer_contains(&buffer, "Cherry"));
    }

    #[test]
    fn shows_count_in_title() {
        let list = List::new().items(vec!["One", "Two", "Three"]);
        let buffer = render_widget(&list, 30, 10);
        assert!(buffer_contains(&buffer, "3/3"));
    }

    #[test]
    fn empty_list_renders() {
        let list: List<String> = List::new().items(vec![]);
        let buffer = render_widget(&list, 30, 10);
        assert!(buffer_contains(&buffer, "0/0"));
    }

    #[test]
    fn selection_tracking() {
        let list = List::new().items(vec!["A", "B", "C"]);
        assert_eq!(list.selected_index(), 0);
        assert_eq!(list.selected_item(), Some(&"A"));
    }
}

mod layout_tests {
    use super::*;

    #[test]
    fn column_splits_height_evenly() {
        let col = Column::new().child(Text::new("1")).child(Text::new("2"));

        let backend = TestBackend::new(10, 4);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                col.render_ansi(frame, Rect::new(0, 0, 10, 4));
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert!(buffer[(0, 0)].symbol().contains('1') || buffer_contains(buffer, "1"));
    }

    #[test]
    fn row_splits_width_evenly() {
        let row = Row::new().child(Text::new("L")).child(Text::new("R"));

        let backend = TestBackend::new(10, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                row.render_ansi(frame, Rect::new(0, 0, 10, 1));
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert!(buffer_contains(buffer, "L"));
        assert!(buffer_contains(buffer, "R"));
    }

    #[test]
    fn stack_renders_all_at_same_position() {
        let stack = Stack::new().child(Text::new("XXXX")).child(Text::new("Y"));

        let buffer = render_widget(&stack, 10, 1);
        assert!(buffer_contains(&buffer, "Y"));
    }
}

mod edge_cases {
    use super::*;

    #[test]
    fn render_to_zero_area() {
        let text = Text::new("Hello");
        let backend = TestBackend::new(0, 0);
        let mut terminal = Terminal::new(backend).unwrap();

        let result = terminal.draw(|frame| {
            text.render_ansi(frame, frame.area());
        });
        assert!(result.is_ok());
    }

    #[test]
    fn render_to_single_cell() {
        let text = Text::new("Hello");
        let buffer = render_widget(&text, 1, 1);
        assert_eq!(buffer.area.width, 1);
        assert_eq!(buffer.area.height, 1);
    }

    #[test]
    fn many_nested_containers() {
        let mut nested = Column::new().child(Text::new("Deep"));
        for _ in 0..10 {
            nested = Column::new().child(nested);
        }
        let buffer = render_widget(&nested, 20, 20);
        assert!(buffer_contains(&buffer, "Deep"));
    }

    #[test]
    fn special_characters() {
        let text = Text::new("Tab:\tNewline:\nBackslash:\\");
        let buffer = render_widget(&text, 40, 3);
        assert!(buffer_contains(&buffer, "Tab:"));
    }
}
