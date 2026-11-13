//! Application-owned semantic actions and their native routing identity.

use serde::{Deserialize, Serialize};

use crate::{FrameId, RegionId};

/// Maximum UTF-8 bytes in an action's widget identity.
pub const MAX_ACTION_WIDGET_ID_BYTES: usize = 256;
/// Maximum UTF-8 bytes in an action refusal.
pub const MAX_ACTION_ERROR_BYTES: usize = 1024;
/// Metadata carrying a widget identity, separate from its native reconciliation key.
pub const REGION_WIDGET_ID_METADATA_KEY: &str = "ulttui.widget_id";
/// Metadata listing explicitly supported actions, separated by commas.
pub const REGION_ACTIONS_METADATA_KEY: &str = "ulttui.actions";
/// Declared focus action bit.
pub const ACTION_FOCUS: u8 = 1;
/// Declared activation action bit.
pub const ACTION_ACTIVATE: u8 = 2;

/// An action the application can accept for a rendered widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum SemanticActionKind {
    /// Move the application's keyboard focus to the widget.
    Focus = 0,
    /// Dispatch the application's explicitly registered activation handler.
    Activate = 1,
}

impl SemanticActionKind {
    /// The action represented by a wire byte, if recognized.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Focus),
            1 => Some(Self::Activate),
            _ => None,
        }
    }

    /// Stable spelling used in rendered action declarations.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Activate => "activate",
        }
    }

    /// Bit used by a rendered action declaration.
    pub const fn mask(self) -> u8 {
        match self {
            Self::Focus => ACTION_FOCUS,
            Self::Activate => ACTION_ACTIVATE,
        }
    }
}

/// A correlated action against one committed app frame and exact widget.
///
/// Frame IDs must increase without reuse for the life of a connection that
/// negotiates semantic actions. This ID differs from the terminal's displayed
/// scene generation, which the external caller supplies to the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticActionRequest {
    /// Terminal-owned correlation identity, unique among pending requests.
    pub request_id: u32,
    /// Last committed application frame observed by the terminal.
    pub frame_id: FrameId,
    /// Region containing the widget in that frame.
    pub id: RegionId,
    /// Exact application widget identity, independent of native keys.
    pub widget_id: String,
    /// Requested operation.
    pub action: SemanticActionKind,
}

/// Whether a widget identity fits the bounded semantic action contract.
pub fn valid_action_widget_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ACTION_WIDGET_ID_BYTES
        && !value.chars().any(char::is_control)
}

/// Read an unambiguous, bounded widget/action declaration from region metadata.
/// Unknown action names, repeated keys or actions, and malformed IDs refuse the
/// entire declaration. An empty action list declares no available operations.
pub fn declared_actions(metadata: &[(String, String)]) -> Option<(&str, u8)> {
    let mut widget_id = None;
    let mut actions = None;
    for (key, value) in metadata {
        match key.as_str() {
            REGION_WIDGET_ID_METADATA_KEY if widget_id.replace(value.as_str()).is_some() => {
                return None
            }
            REGION_ACTIONS_METADATA_KEY if actions.replace(value.as_str()).is_some() => {
                return None
            }
            _ => {}
        }
    }
    let widget_id = widget_id.filter(|id| valid_action_widget_id(id))?;
    let mut mask = 0;
    let actions = actions?;
    if !actions.is_empty() {
        for action in actions.split(',') {
            let bit = match action {
                "focus" => ACTION_FOCUS,
                "activate" => ACTION_ACTIVATE,
                _ => return None,
            };
            if mask & bit != 0 {
                return None;
            }
            mask |= bit;
        }
    }
    Some((widget_id, mask))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppMessage, Decode, Encode, TerminalMessage};

    #[test]
    fn test_action_metadata_is_strict_and_unambiguous() {
        let metadata = vec![
            (REGION_WIDGET_ID_METADATA_KEY.into(), "submit".into()),
            (REGION_ACTIONS_METADATA_KEY.into(), "focus,activate".into()),
        ];
        assert_eq!(declared_actions(&metadata), Some(("submit", 3)));
        for actions in [
            "activate,activate",
            "click",
            "focus,",
            " focus",
            "focus,activate,unknown",
        ] {
            let mut bad = metadata.clone();
            bad[1].1 = actions.into();
            assert_eq!(declared_actions(&bad), None);
        }
        let mut duplicate = metadata.clone();
        duplicate.push(metadata[0].clone());
        assert_eq!(declared_actions(&duplicate), None);
        assert!(!valid_action_widget_id(""));
        assert!(!valid_action_widget_id("bad\nidentity"));
        assert!(!valid_action_widget_id(&"x".repeat(257)));
    }

    #[test]
    fn test_action_wire_roundtrip_and_refusal() {
        for action in [SemanticActionKind::Focus, SemanticActionKind::Activate] {
            let message = TerminalMessage::InvokeAction(SemanticActionRequest {
                request_id: 7,
                frame_id: FrameId(42),
                id: RegionId(8),
                widget_id: "submit".into(),
                action,
            });
            let bytes = message.encode_to_bytes();
            assert_eq!(TerminalMessage::decode(&mut &bytes[..]).unwrap(), message);
            let mut unknown = bytes.to_vec();
            *unknown.last_mut().unwrap() = 255;
            assert!(TerminalMessage::decode(&mut &unknown[..]).is_err());
        }
        for error in [None, Some("application frame changed".into())] {
            let message = AppMessage::ActionResult {
                request_id: 7,
                error,
            };
            assert_eq!(
                AppMessage::decode(&mut &message.encode_to_bytes()[..]).unwrap(),
                message
            );
        }
        for widget_id in ["".into(), "x".repeat(257), "bad\nidentity".into()] {
            let bytes = TerminalMessage::InvokeAction(SemanticActionRequest {
                request_id: 7,
                frame_id: FrameId(42),
                id: RegionId(8),
                widget_id,
                action: SemanticActionKind::Activate,
            })
            .encode_to_bytes();
            assert!(TerminalMessage::decode(&mut &bytes[..]).is_err());
        }
        for error in ["x".repeat(1025), "bad\nrefusal".into()] {
            let bytes = AppMessage::ActionResult {
                request_id: 7,
                error: Some(error),
            }
            .encode_to_bytes();
            assert!(AppMessage::decode(&mut &bytes[..]).is_err());
        }
        assert_eq!(
            crate::CLIENT_DEFAULT_CAPABILITIES & crate::CAP_SEMANTIC_ACTIONS,
            0
        );
    }
}
