//! Wayland global hotkey registration via `org.freedesktop.portal.GlobalShortcuts`
//! (ADR 0028 Slice 4).
//!
//! Uses `zbus` over the session D-Bus on a dedicated background thread so
//! compositor permission dialogs never block Ultty's UI loop, forwards the
//! portal's `activation_token` when the shortcut triggers, closes the portal
//! session cleanly on drop/reload, and logs a graceful fallback message on
//! compositors that do not implement the portal (such as sway).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use futures_core::Stream;
use thiserror::Error;
use tokio::sync::oneshot;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

use crate::config::KeybindingConfig;

const PORTAL_DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const GLOBAL_SHORTCUTS_INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";
const SESSION_INTERFACE: &str = "org.freedesktop.portal.Session";
const SHORTCUT_ID: &str = "toggle_quick_terminal";
const SHORTCUT_DESCRIPTION: &str = "Toggle Ultty quick terminal";
const CREATE_SESSION_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_SESSION_TIMEOUT: Duration = Duration::from_millis(500);

static TOKEN_SEQ: AtomicU64 = AtomicU64::new(1);

/// Errors returned while validating or connecting to the Wayland
/// `GlobalShortcuts` portal.
#[derive(Debug, Error)]
pub enum PortalHotkeyError {
    /// The binding string did not parse under `[keybindings]` syntax.
    #[error("invalid hotkey syntax '{binding}' (expected e.g. 'ctrl+grave' or 'super+grave')")]
    InvalidSyntax { binding: String },
    /// The key name is not mapped to an XKB keysym name.
    #[error("unsupported key '{key}' in hotkey '{binding}'")]
    UnsupportedKey { binding: String, key: String },
    /// Session D-Bus is unavailable.
    #[error("session D-Bus is unavailable: {0}")]
    NoSessionBus(String),
    /// The compositor or portal does not implement `GlobalShortcuts`.
    #[error("org.freedesktop.portal.GlobalShortcuts is not available on this desktop")]
    Unsupported,
    /// The user or compositor declined the shortcut request.
    #[error("GlobalShortcuts portal request was declined (response {code})")]
    Cancelled { code: u32 },
    /// Any other D-Bus / portal error.
    #[error("GlobalShortcuts portal error: {0}")]
    Protocol(String),
}

/// Active handle to a background `org.freedesktop.portal.GlobalShortcuts`
/// listener. Dropping it cancels the listener, closes the portal session, and
/// joins the thread.
pub struct WaylandPortalHotkey {
    cancel_tx: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl WaylandPortalHotkey {
    /// Validate `binding` under `[keybindings]` syntax and start registering it
    /// with `org.freedesktop.portal.GlobalShortcuts` in the background.
    ///
    /// Whenever the shortcut fires, `on_trigger` is called with the optional
    /// Wayland `activation_token` supplied by the portal.
    pub fn start<F>(binding: &str, on_trigger: F) -> Result<Self, PortalHotkeyError>
    where
        F: Fn(Option<String>) + Send + 'static,
    {
        let trigger = keybinding_to_portal_trigger(binding)?;
        let binding_owned = binding.to_owned();
        let (cancel_tx, cancel_rx) = oneshot::channel();

        let thread = thread::Builder::new()
            .name("ultty-portal-hotkey".to_string())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                else {
                    tracing::warn!("Failed to start runtime for GlobalShortcuts portal");
                    return;
                };
                runtime.block_on(run_portal_worker(
                    binding_owned,
                    trigger,
                    cancel_rx,
                    on_trigger,
                ));
            })
            .map_err(|e| PortalHotkeyError::Protocol(format!("failed to spawn thread: {e}")))?;

        Ok(Self {
            cancel_tx: Some(cancel_tx),
            thread: Some(thread),
        })
    }
}

impl Drop for WaylandPortalHotkey {
    fn drop(&mut self) {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

async fn run_portal_worker<F>(
    binding: String,
    trigger: String,
    mut cancel_rx: oneshot::Receiver<()>,
    on_trigger: F,
) where
    F: Fn(Option<String>),
{
    let conn = tokio::select! {
        _ = &mut cancel_rx => return,
        res = Connection::session() => match res {
            Ok(conn) => conn,
            Err(err) => {
                tracing::info!(
                    hotkey = %binding,
                    error = %err,
                    "GlobalShortcuts portal unavailable (no session D-Bus); bind `ultty --quick` in your compositor (see docs/quick-terminal.md)"
                );
                return;
            }
        }
    };

    let session_path = match create_portal_session(&conn, &mut cancel_rx).await {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(PortalHotkeyError::Unsupported) => {
            tracing::info!(
                hotkey = %binding,
                "org.freedesktop.portal.GlobalShortcuts is not supported on this desktop; bind `ultty --quick` in your compositor (see docs/quick-terminal.md)"
            );
            return;
        }
        Err(err) => {
            tracing::warn!(
                hotkey = %binding,
                "Failed to open GlobalShortcuts portal session: {err}"
            );
            return;
        }
    };

    match bind_portal_shortcut(&conn, &session_path, &trigger, &mut cancel_rx).await {
        Ok(true) => {
            tracing::info!(
                hotkey = %binding,
                trigger = %trigger,
                "Registered quick terminal hotkey via GlobalShortcuts portal"
            );
        }
        Ok(false) => {
            close_portal_session(&conn, &session_path).await;
            return;
        }
        Err(PortalHotkeyError::Unsupported) => {
            tracing::info!(
                hotkey = %binding,
                "org.freedesktop.portal.GlobalShortcuts is not supported on this desktop; bind `ultty --quick` in your compositor (see docs/quick-terminal.md)"
            );
            close_portal_session(&conn, &session_path).await;
            return;
        }
        Err(PortalHotkeyError::Cancelled { code }) => {
            tracing::info!(
                hotkey = %binding,
                code,
                "GlobalShortcuts portal binding was declined or cancelled; bind `ultty --quick` in your compositor if needed"
            );
            close_portal_session(&conn, &session_path).await;
            return;
        }
        Err(err) => {
            tracing::warn!(
                hotkey = %binding,
                "Failed to bind quick terminal hotkey via GlobalShortcuts portal: {err}"
            );
            close_portal_session(&conn, &session_path).await;
            return;
        }
    }

    if let Ok(portal) = Proxy::new(
        &conn,
        PORTAL_DESTINATION,
        PORTAL_PATH,
        GLOBAL_SHORTCUTS_INTERFACE,
    )
    .await
    {
        if let Ok(mut activated) = portal.receive_signal("Activated").await {
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => break,
                    maybe_msg = next_signal(&mut activated) => {
                        let Some(msg) = maybe_msg else {
                            break;
                        };
                        if let Ok((sig_session, shortcut_id, _timestamp, options)) = msg
                            .body()
                            .deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>()
                        {
                            if sig_session == session_path && shortcut_id == SHORTCUT_ID {
                                let token = extract_activation_token(&options);
                                on_trigger(token);
                            }
                        }
                    }
                }
            }
        }
    }

    close_portal_session(&conn, &session_path).await;
}

async fn create_portal_session(
    conn: &Connection,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> Result<Option<OwnedObjectPath>, PortalHotkeyError> {
    let unique = conn
        .unique_name()
        .ok_or_else(|| PortalHotkeyError::Protocol("missing D-Bus unique name".to_string()))?;
    let handle_token = next_token("cs");
    let session_token = next_token("qs");
    let expected_request_path = request_handle_path(unique.as_str(), &handle_token);

    let req_proxy = Proxy::new(
        conn,
        PORTAL_DESTINATION,
        expected_request_path.as_str(),
        REQUEST_INTERFACE,
    )
    .await
    .map_err(|e| classify_portal_error(&e))?;
    let mut responses = req_proxy
        .receive_signal("Response")
        .await
        .map_err(|e| classify_portal_error(&e))?;

    let portal = Proxy::new(
        conn,
        PORTAL_DESTINATION,
        PORTAL_PATH,
        GLOBAL_SHORTCUTS_INTERFACE,
    )
    .await
    .map_err(|e| classify_portal_error(&e))?;

    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(handle_token.as_str()));
    options.insert("session_handle_token", Value::from(session_token.as_str()));

    let body = (options,);
    let call = portal.call_method("CreateSession", &body);
    let reply = tokio::select! {
        _ = &mut *cancel_rx => return Ok(None),
        res = tokio::time::timeout(CREATE_SESSION_TIMEOUT, call) => match res {
            Ok(Ok(msg)) => msg,
            Ok(Err(err)) => return Err(classify_portal_error(&err)),
            Err(_) => return Err(PortalHotkeyError::Protocol("CreateSession timed out".to_string())),
        }
    };
    let _returned_handle: OwnedObjectPath = reply
        .body()
        .deserialize()
        .map_err(|e| PortalHotkeyError::Protocol(format!("invalid CreateSession reply: {e}")))?;

    let response_msg = tokio::select! {
        _ = &mut *cancel_rx => return Ok(None),
        res = tokio::time::timeout(CREATE_SESSION_TIMEOUT, next_signal(&mut responses)) => match res {
            Ok(Some(msg)) => msg,
            Ok(None) => return Err(PortalHotkeyError::Protocol("Response stream closed".to_string())),
            Err(_) => return Err(PortalHotkeyError::Protocol("CreateSession Response timed out".to_string())),
        }
    };

    let (code, results): (u32, HashMap<String, OwnedValue>) = response_msg
        .body()
        .deserialize()
        .map_err(|e| PortalHotkeyError::Protocol(format!("invalid Response payload: {e}")))?;
    if code != 0 {
        return Err(PortalHotkeyError::Cancelled { code });
    }

    let handle_str = results
        .get("session_handle")
        .and_then(|v| {
            v.downcast_ref::<String>().ok().or_else(|| {
                v.downcast_ref::<zbus::zvariant::ObjectPath<'_>>()
                    .ok()
                    .map(|p| p.as_str().to_owned())
            })
        })
        .unwrap_or_else(|| session_handle_path(unique.as_str(), &session_token));

    let session_path = OwnedObjectPath::try_from(handle_str)
        .map_err(|e| PortalHotkeyError::Protocol(format!("invalid session_handle path: {e}")))?;
    Ok(Some(session_path))
}

async fn bind_portal_shortcut(
    conn: &Connection,
    session_path: &OwnedObjectPath,
    trigger: &str,
    cancel_rx: &mut oneshot::Receiver<()>,
) -> Result<bool, PortalHotkeyError> {
    let unique = conn
        .unique_name()
        .ok_or_else(|| PortalHotkeyError::Protocol("missing D-Bus unique name".to_string()))?;
    let handle_token = next_token("bs");
    let expected_request_path = request_handle_path(unique.as_str(), &handle_token);

    let req_proxy = Proxy::new(
        conn,
        PORTAL_DESTINATION,
        expected_request_path.as_str(),
        REQUEST_INTERFACE,
    )
    .await
    .map_err(|e| classify_portal_error(&e))?;
    let mut responses = req_proxy
        .receive_signal("Response")
        .await
        .map_err(|e| classify_portal_error(&e))?;

    let portal = Proxy::new(
        conn,
        PORTAL_DESTINATION,
        PORTAL_PATH,
        GLOBAL_SHORTCUTS_INTERFACE,
    )
    .await
    .map_err(|e| classify_portal_error(&e))?;

    let mut shortcut_opts: HashMap<&str, Value<'_>> = HashMap::new();
    shortcut_opts.insert("description", Value::from(SHORTCUT_DESCRIPTION));
    shortcut_opts.insert("preferred_trigger", Value::from(trigger));
    let shortcuts: Vec<(&str, HashMap<&str, Value<'_>>)> = vec![(SHORTCUT_ID, shortcut_opts)];

    let mut call_opts: HashMap<&str, Value<'_>> = HashMap::new();
    call_opts.insert("handle_token", Value::from(handle_token.as_str()));

    let body = (session_path, shortcuts, "", call_opts);
    let call = portal.call_method("BindShortcuts", &body);
    tokio::select! {
        _ = &mut *cancel_rx => return Ok(false),
        res = tokio::time::timeout(CREATE_SESSION_TIMEOUT, call) => match res {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => return Err(classify_portal_error(&err)),
            Err(_) => return Err(PortalHotkeyError::Protocol("BindShortcuts call timed out".to_string())),
        }
    }

    let response_msg = tokio::select! {
        _ = &mut *cancel_rx => return Ok(false),
        maybe = next_signal(&mut responses) => match maybe {
            Some(msg) => msg,
            None => return Err(PortalHotkeyError::Protocol("BindShortcuts Response stream closed".to_string())),
        }
    };

    let (code, _results): (u32, HashMap<String, OwnedValue>) = response_msg
        .body()
        .deserialize()
        .map_err(|e| PortalHotkeyError::Protocol(format!("invalid BindShortcuts response: {e}")))?;
    if code != 0 {
        return Err(PortalHotkeyError::Cancelled { code });
    }
    Ok(true)
}

async fn close_portal_session(conn: &Connection, session_path: &OwnedObjectPath) {
    let Ok(session) = Proxy::new(
        conn,
        PORTAL_DESTINATION,
        session_path.as_str(),
        SESSION_INTERFACE,
    )
    .await
    else {
        return;
    };
    let _ = tokio::time::timeout(CLOSE_SESSION_TIMEOUT, session.call_method("Close", &())).await;
}

async fn next_signal(stream: &mut zbus::proxy::SignalStream<'_>) -> Option<zbus::Message> {
    std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await
}

fn next_token(prefix: &str) -> String {
    let seq = TOKEN_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("ultty_{prefix}_{}_{seq}", std::process::id())
}

fn sanitize_bus_sender(unique_name: &str) -> String {
    unique_name.trim_start_matches(':').replace('.', "_")
}

/// Build the XDG Desktop Portal `Request` object path for `unique_name` and `token`.
pub fn request_handle_path(unique_name: &str, token: &str) -> String {
    format!(
        "/org/freedesktop/portal/desktop/request/{}/{}",
        sanitize_bus_sender(unique_name),
        token
    )
}

fn session_handle_path(unique_name: &str, token: &str) -> String {
    format!(
        "/org/freedesktop/portal/desktop/session/{}/{}",
        sanitize_bus_sender(unique_name),
        token
    )
}

/// Extract a non-empty `activation_token` string from a portal `Activated`
/// signal's options dictionary.
pub fn extract_activation_token(options: &HashMap<String, OwnedValue>) -> Option<String> {
    let raw = options
        .get("activation_token")?
        .downcast_ref::<String>()
        .ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Classify a `zbus` error into [`PortalHotkeyError`], distinguishing missing
/// portal support from unexpected D-Bus errors.
pub fn classify_portal_error(err: &zbus::Error) -> PortalHotkeyError {
    if let zbus::Error::MethodError(name, _, _) = err {
        if is_unsupported_dbus_error(name.as_str()) {
            return PortalHotkeyError::Unsupported;
        }
    }
    if matches!(err, zbus::Error::InputOutput(_) | zbus::Error::Address(_)) {
        return PortalHotkeyError::NoSessionBus(err.to_string());
    }
    PortalHotkeyError::Protocol(err.to_string())
}

fn is_unsupported_dbus_error(name: &str) -> bool {
    matches!(
        name,
        "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
    )
}

/// Convert a `[keybindings]` string (such as `"ctrl+grave"` or `"super+space"`)
/// into an XDG Shortcuts Spec `preferred_trigger` string (such as `"CTRL+grave"`
/// or `"LOGO+space"`).
pub fn keybinding_to_portal_trigger(binding: &str) -> Result<String, PortalHotkeyError> {
    let parsed = KeybindingConfig::parse_keybinding(binding).ok_or_else(|| {
        PortalHotkeyError::InvalidSyntax {
            binding: binding.to_owned(),
        }
    })?;
    let xkb_key =
        key_name_to_xkb_name(&parsed.key).ok_or_else(|| PortalHotkeyError::UnsupportedKey {
            binding: binding.to_owned(),
            key: parsed.key.clone(),
        })?;

    let mut parts: Vec<String> = Vec::with_capacity(5);
    if parsed.ctrl {
        parts.push("CTRL".to_string());
    }
    if parsed.alt {
        parts.push("ALT".to_string());
    }
    if parsed.shift {
        parts.push("SHIFT".to_string());
    }
    if parsed.super_key {
        parts.push("LOGO".to_string());
    }
    parts.push(xkb_key);
    Ok(parts.join("+"))
}

fn key_name_to_xkb_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.len() == 1 {
        let b = name.as_bytes()[0];
        if b.is_ascii_lowercase() || b.is_ascii_digit() {
            return Some(name.to_owned());
        }
    }
    if let Some(rest) = name.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(format!("F{n}"));
            }
        }
    }
    let mapped = match name {
        "grave" | "`" | "backquote" | "tilde" | "~" => "grave",
        "space" | " " => "space",
        "tab" => "Tab",
        "enter" | "return" => "Return",
        "escape" | "esc" => "Escape",
        "backspace" => "BackSpace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page_up" => "Page_Up",
        "pagedown" | "page_down" => "Page_Down",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "minus" | "-" => "minus",
        "equal" | "equals" | "=" => "equal",
        "plus" | "+" => "plus",
        "bracketleft" | "[" => "bracketleft",
        "bracketright" | "]" => "bracketright",
        "backslash" | "\\" => "backslash",
        "semicolon" | ";" => "semicolon",
        "apostrophe" | "'" | "quote" => "apostrophe",
        "comma" | "," => "comma",
        "period" | "." => "period",
        "slash" | "/" => "slash",
        _ => return None,
    };
    Some(mapped.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybinding_converts_to_xdg_shortcuts_trigger() {
        assert_eq!(
            keybinding_to_portal_trigger("ctrl+grave").unwrap(),
            "CTRL+grave"
        );
        assert_eq!(
            keybinding_to_portal_trigger("super+grave").unwrap(),
            "LOGO+grave"
        );
        assert_eq!(
            keybinding_to_portal_trigger("alt+space").unwrap(),
            "ALT+space"
        );
        assert_eq!(
            keybinding_to_portal_trigger("ctrl+shift+f12").unwrap(),
            "CTRL+SHIFT+F12"
        );
        assert_eq!(keybinding_to_portal_trigger("f12").unwrap(), "F12");
        assert_eq!(
            keybinding_to_portal_trigger("cmd+enter").unwrap(),
            "LOGO+Return"
        );
        assert!(matches!(
            keybinding_to_portal_trigger("bogus+grave"),
            Err(PortalHotkeyError::InvalidSyntax { .. })
        ));
        assert!(matches!(
            keybinding_to_portal_trigger("ctrl+hyperkey"),
            Err(PortalHotkeyError::UnsupportedKey { .. })
        ));
    }

    #[test]
    fn request_handle_path_sanitizes_bus_unique_name() {
        assert_eq!(
            request_handle_path(":1.42", "ultty_cs_1_1"),
            "/org/freedesktop/portal/desktop/request/1_42/ultty_cs_1_1"
        );
        assert_eq!(
            session_handle_path(":1.42", "ultty_qs_1_2"),
            "/org/freedesktop/portal/desktop/session/1_42/ultty_qs_1_2"
        );
    }

    #[test]
    fn activation_token_extraction_trims_and_ignores_empty() {
        let mut opts = HashMap::new();
        assert_eq!(extract_activation_token(&opts), None);

        opts.insert(
            "activation_token".to_string(),
            OwnedValue::from(zbus::zvariant::Str::from("   ")),
        );
        assert_eq!(extract_activation_token(&opts), None);

        opts.insert(
            "activation_token".to_string(),
            OwnedValue::from(zbus::zvariant::Str::from("  tok-abc  ")),
        );
        assert_eq!(extract_activation_token(&opts).as_deref(), Some("tok-abc"));
    }

    #[test]
    fn unsupported_dbus_error_names_are_classified_cleanly() {
        for name in [
            "org.freedesktop.DBus.Error.UnknownMethod",
            "org.freedesktop.DBus.Error.UnknownInterface",
            "org.freedesktop.DBus.Error.UnknownObject",
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
        ] {
            assert!(is_unsupported_dbus_error(name), "{name}");
        }
        assert!(!is_unsupported_dbus_error(
            "org.freedesktop.DBus.Error.Failed"
        ));
    }

    #[test]
    fn start_rejects_invalid_syntax_and_unknown_keys_synchronously() {
        assert!(matches!(
            WaylandPortalHotkey::start("bogus+grave", |_| {}),
            Err(PortalHotkeyError::InvalidSyntax { .. })
        ));
        assert!(matches!(
            WaylandPortalHotkey::start("ctrl+hyperkey", |_| {}),
            Err(PortalHotkeyError::UnsupportedKey { .. })
        ));
    }
}
