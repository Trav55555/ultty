//! Layout helpers.

// Re-export container widgets as layout primitives
pub use crate::widgets::{Column, Row, Stack};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{Text, View};
    use ultty_protocol::{Dimension, Display, FlexDirection};

    // --- Column layout ---

    #[test]
    fn column_new_is_flex_column() {
        let col = Column::new();
        let c = <Column as View>::constraints(&col);
        assert_eq!(c.display, Display::Flex);
        assert_eq!(c.flex_direction, FlexDirection::Column);
    }

    #[test]
    fn column_empty_has_no_children() {
        let col = Column::new();
        assert_eq!(<Column as View>::children_count(&col), 0);
    }

    #[test]
    fn column_with_children() {
        let col = Column::new().child(Text::new("a")).child(Text::new("b"));
        assert_eq!(<Column as View>::children_count(&col), 2);
    }

    #[test]
    fn column_gap_sets_constraint() {
        let col = Column::new().gap(3.0);
        let c = <Column as View>::constraints(&col);
        assert_eq!(c.gap.height, Dimension::Cells(3.0));
    }

    #[test]
    fn column_padding_sets_constraint() {
        let col = Column::new().padding(2.0);
        let c = <Column as View>::constraints(&col);
        assert_eq!(c.padding.top, Dimension::Cells(2.0));
        assert_eq!(c.padding.right, Dimension::Cells(2.0));
        assert_eq!(c.padding.bottom, Dimension::Cells(2.0));
        assert_eq!(c.padding.left, Dimension::Cells(2.0));
    }

    #[test]
    fn column_default_is_new() {
        let col = Column::default();
        assert_eq!(<Column as View>::children_count(&col), 0);
    }

    #[test]
    fn column_children_count_method() {
        let col = Column::new()
            .child(Text::new("x"))
            .child(Text::new("y"))
            .child(Text::new("z"));
        assert_eq!(col.children_count(), 3);
    }

    #[test]
    fn column_is_debuggable() {
        let col = Column::new().child(Text::new("a"));
        let s = format!("{:?}", col);
        assert!(s.contains("Column"));
    }

    // --- Row layout ---

    #[test]
    fn row_new_is_flex_row() {
        let row = Row::new();
        let c = <Row as View>::constraints(&row);
        assert_eq!(c.display, Display::Flex);
        assert_eq!(c.flex_direction, FlexDirection::Row);
    }

    #[test]
    fn row_empty_has_no_children() {
        let row = Row::new();
        assert_eq!(<Row as View>::children_count(&row), 0);
    }

    #[test]
    fn row_with_children() {
        let row = Row::new()
            .child(Text::new("a"))
            .child(Text::new("b"))
            .child(Text::new("c"));
        assert_eq!(<Row as View>::children_count(&row), 3);
    }

    #[test]
    fn row_gap_sets_constraint() {
        let row = Row::new().gap(5.0);
        let c = <Row as View>::constraints(&row);
        assert_eq!(c.gap.width, Dimension::Cells(5.0));
    }

    #[test]
    fn row_default_is_new() {
        let row = Row::default();
        assert_eq!(<Row as View>::children_count(&row), 0);
    }

    #[test]
    fn row_children_count_method() {
        let row = Row::new().child(Text::new("a")).child(Text::new("b"));
        assert_eq!(row.children_count(), 2);
    }

    #[test]
    fn row_is_debuggable() {
        let row = Row::new();
        let s = format!("{:?}", row);
        assert!(s.contains("Row"));
    }

    // --- Stack layout ---

    #[test]
    fn stack_new_has_default_constraints() {
        let stack = Stack::new();
        let c = <Stack as View>::constraints(&stack);
        assert_eq!(c, ultty_protocol::Constraints::default());
    }

    #[test]
    fn stack_empty_has_no_children() {
        let stack = Stack::new();
        assert_eq!(<Stack as View>::children_count(&stack), 0);
    }

    #[test]
    fn stack_with_children() {
        let stack = Stack::new()
            .child(Text::new("layer1"))
            .child(Text::new("layer2"));
        assert_eq!(<Stack as View>::children_count(&stack), 2);
    }

    #[test]
    fn stack_default_is_new() {
        let stack = Stack::default();
        assert_eq!(<Stack as View>::children_count(&stack), 0);
    }

    #[test]
    fn stack_children_count_method() {
        let stack = Stack::new().child(Text::new("only"));
        assert_eq!(stack.children_count(), 1);
    }

    #[test]
    fn stack_is_debuggable() {
        let stack = Stack::new();
        let s = format!("{:?}", stack);
        assert!(s.contains("Stack"));
    }

    // --- Nested layouts ---

    #[test]
    fn nested_column_in_row() {
        let layout = Row::new()
            .child(Column::new().child(Text::new("a")).child(Text::new("b")))
            .child(Text::new("c"));
        assert_eq!(<Row as View>::children_count(&layout), 2);
    }

    #[test]
    fn nested_row_in_column() {
        let layout = Column::new()
            .child(Row::new().child(Text::new("a")).child(Text::new("b")))
            .child(Text::new("c"));
        assert_eq!(<Column as View>::children_count(&layout), 2);
    }

    #[test]
    fn stack_with_nested_containers() {
        let layout = Stack::new()
            .child(Column::new().child(Text::new("bg")))
            .child(Row::new().child(Text::new("fg")));
        assert_eq!(<Stack as View>::children_count(&layout), 2);
    }

    // --- Layout components are not focusable ---

    #[test]
    fn column_not_focusable() {
        let col = Column::new();
        assert!(!col.is_focusable());
    }

    #[test]
    fn row_not_focusable() {
        let row = Row::new();
        assert!(!row.is_focusable());
    }

    #[test]
    fn stack_not_focusable() {
        let stack = Stack::new();
        assert!(!stack.is_focusable());
    }
}
