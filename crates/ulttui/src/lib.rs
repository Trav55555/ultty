//! # ultimate-tui
//!
//! A TUI framework that works in any terminal, with native acceleration
//! when running in Ultty.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use ulttui::prelude::*;
//!
//! struct App { count: i32 }
//!
//! impl Application for App {
//!     type Message = ();
//!     
//!     fn view(&self) -> impl View {
//!         Text::new(format!("Count: {}", self.count))
//!     }
//! }
//!
//! fn main() {
//!     App { count: 0 }.run().unwrap();
//! }
//! ```
//!
//! ## Features
//!
//! - **Dual-mode rendering**: Native protocol in Ultty, ANSI fallback elsewhere
//! - **Declarative UI**: Build UIs as functions of state
//! - **Elm architecture**: Message → Update → View
//! - **Flexible layouts**: Flexbox and Grid via Taffy
//! - **Built-in widgets**: Text, Input, Button, ScrollView, etc.

pub mod app;
pub mod backend;
pub mod cmd;
pub mod event;
pub mod focus;
pub mod layout;
pub mod screen;
pub mod theme;
pub mod toast;
pub mod vim;
pub mod widgets;

/// Prelude for convenient imports.
pub mod prelude {
    pub use crate::app::{App, Application};
    pub use crate::cmd::{Cmd, Sub};
    pub use crate::event::Event;
    pub use crate::focus::FocusManager;
    pub use crate::layout::{Column, Row};
    pub use crate::screen::{Screen, ScreenAction};
    pub use crate::theme::Theme;
    pub use crate::toast::{Toast, ToastOverlay, ToastSeverity};
    pub use crate::vim::{VimConfig, VimExt, VimHandler, VimResult};
    pub use crate::widgets::{
        Accent, Badge, BadgeVariant, Body, Button, Card, Collapsible, Divider, DockLayout, Inset,
        List, MarkdownView, Modal, Muted, ProgressBar, ScrollView, Section, Spacer, Spinner, Table,
        Tabs, Text, TextInput, Title, View,
    };

    // Re-export protocol types
    pub use ultty_protocol::{Color, Constraints, Dimension, Style};

    // Re-export vim types from protocol
    pub use ultty_protocol::vim::{VimAction, VimCommand, VimMode, VimMotion, VimOperator};
}
