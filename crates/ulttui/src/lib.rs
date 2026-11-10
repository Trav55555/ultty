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
//! - **Dual-mode rendering**: Ultty-native mode in Ultty, ANSI mode on supported Unix terminals
//! - **Declarative UI**: Build UIs as functions of state
//! - **Elm architecture**: Message → Update → View
//! - **Flexible layouts**: Flexbox and Grid via Taffy
//! - **Built-in widgets**: Text, Input, Button, ScrollView, etc.

#[cfg(not(unix))]
compile_error!(
    "ulttui currently targets Unix terminals. Windows support requires a cfg-gated ANSI/native backend split."
);

pub mod app;
pub mod backend;
pub mod cmd;
pub mod event;
pub mod focus;
pub mod inline;
pub mod layout;
pub mod screen;
pub mod theme;
pub mod toast;
pub mod vim;
pub mod widgets;

mod error;
pub use error::{Error, Result};

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
        Accent, Badge, BadgeVariant, Body, Button, Card, Collapsible, DiffKind, DiffLine, DiffMode,
        DiffView, Divider, DockLayout, Inset, List, MarkdownView, Modal, Muted, ProgressBar,
        ScrollView, Section, Semantic, Spacer, Spinner, Table, Tabs, Text, TextInput, Title, Tree,
        TreeNode, View,
    };

    // Re-export protocol types
    pub use ultty_protocol::{
        BorderEdges, BorderStyle, BorderType, Color, Constraints, Dimension, Style,
    };

    // Vim types
    pub use crate::vim::{VimAction, VimCommand, VimMode, VimMotion, VimOperator};
}
