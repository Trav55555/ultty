use crate::event::Event;
use crate::widgets::Column;

pub trait Screen: Send {
    fn view(&self) -> Column;
    fn on_event(&mut self, event: &Event) -> ScreenAction;
    fn on_mount(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenAction {
    Continue,
    Dismiss,
    Consumed,
}
