//! Conversation navigation and local drafts. Opening a conversation never submits a turn.
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ulttui::event::{Event, Key};
use ulttui::prelude::{Cmd, Column, Text};
use ulttui::toast::Toast;

use crate::types::ChatMessage;
use crate::{Msg, PorygonTui};

#[derive(Debug, Deserialize)]
pub(crate) struct Conversation {
    pub(crate) id: String,
    title: Option<String>,
    updated_at: String,
    message_count: usize,
}

#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
struct Position {
    draft: String,
    scroll: f32,
    follow: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct DraftStore {
    positions: BTreeMap<String, Position>,
    active: Option<String>,
    run: Option<String>,
    unresolved: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredDrafts {
    Current(DraftStore),
    Legacy(BTreeMap<String, Position>),
}

#[derive(Default)]
pub(crate) struct Conversations {
    pub(crate) open: bool,
    pub(crate) rows: Vec<Conversation>,
    pub(crate) error: Option<String>,
    query: String,
    selected: usize,
    pending: Option<(String, u64)>,
    generation: u64,
    active: Option<String>,
    run: Option<String>,
    unresolved: bool,
    dirty: bool,
    restoring_history: bool,
    positions: BTreeMap<String, Position>,
    path: Option<PathBuf>,
    lock: Option<PathBuf>,
}

impl Conversations {
    fn filtered(&self) -> Vec<&Conversation> {
        let query = self.query.to_lowercase();
        self.rows
            .iter()
            .filter(|row| {
                row.title
                    .as_deref()
                    .unwrap_or("Untitled conversation")
                    .to_lowercase()
                    .contains(&query)
            })
            .collect()
    }

    fn write(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path.parent().ok_or("Draft path has no parent")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(
            &serde_json::to_vec(&DraftStore {
                positions: self.positions.clone(),
                active: self.active.clone(),
                run: self.run.clone(),
                unresolved: self.unresolved,
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(temporary, path).map_err(|e| e.to_string())
    }
}

impl PorygonTui {
    pub(crate) fn initialize_drafts(&mut self) {
        let root = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            });
        let mut namespace = std::collections::hash_map::DefaultHasher::new();
        self.connection
            .as_ref()
            .map(|connection| connection.base_url.as_str())
            .unwrap_or(&self.daemon_url)
            .hash(&mut namespace);
        std::env::current_dir().ok().hash(&mut namespace);
        self.conversations.path = root.map(|root| {
            root.join(format!(
                "porygon-tui/{:016x}/drafts.json",
                namespace.finish()
            ))
        });
        if let Some(path) = &self.conversations.path {
            match fs::read(path) {
                Ok(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(StoredDrafts::Current(store)) => self.restore_store(store),
                    Ok(StoredDrafts::Legacy(positions)) => self.conversations.positions = positions,
                    Err(error) => {
                        self.error = Some(format!("Could not read drafts; persistence disabled to preserve the file: {error}"));
                        self.conversations.path = None;
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    self.error = Some(format!(
                        "Could not read drafts; persistence disabled to preserve the file: {error}"
                    ));
                    self.conversations.path = None;
                }
            }
        }
        if let Some(path) = &self.conversations.path {
            let lock = path.with_extension("lock");
            let result = (|| -> Result<(), String> {
                fs::create_dir_all(path.parent().ok_or("Missing draft directory")?)
                    .map_err(|error| error.to_string())?;
                if let Ok(pid) = fs::read_to_string(&lock) {
                    if let Ok(pid) = pid.trim().parse::<i32>() {
                        // SAFETY: signal zero checks process existence without delivering a signal.
                        if pid > 0
                            && unsafe { libc::kill(pid, 0) } == -1
                            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                        {
                            fs::remove_file(&lock).map_err(|error| error.to_string())?;
                        }
                    }
                }
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(&lock)
                    .map_err(|error| error.to_string())?;
                write!(file, "{}", std::process::id()).map_err(|error| error.to_string())?;
                Ok(())
            })();
            match result {
                Ok(()) => self.conversations.lock = Some(lock),
                Err(error) => {
                    self.error = Some(format!(
                        "Draft persistence unavailable (another client may own it): {error}"
                    ));
                    self.conversations.path = None;
                }
            }
        }
        self.restore_position();
    }

    fn restore_store(&mut self, store: DraftStore) {
        self.conversations.positions = store.positions;
        self.conversation_id = store.active.clone();
        self.conversations.restoring_history = self.conversation_id.is_some();
        self.current_run = store.run.clone();
        self.run_unresolved = store.unresolved;
        self.conversations.active = store.active;
        self.conversations.run = store.run;
        self.conversations.unresolved = store.unresolved;
        if self.run_unresolved {
            self.status.step = "previous work unconfirmed; /reconnect to inspect".into();
        }
        if self.conversation_id.is_some() {
            self.messages[0].content = "Previous conversation restored with your draft. F2 opens its history. Previous work is never resubmitted.".into();
        }
    }

    pub(crate) fn assign_conversation(&mut self, id: String) {
        if self.conversation_id.is_none() {
            self.conversations.positions.remove("");
        }
        self.conversation_id = Some(id);
        self.persist_conversation();
    }

    pub(crate) fn flush_drafts(&mut self) {
        self.persist_conversation();
        if self.conversations.dirty {
            match self.conversations.write() {
                Ok(()) => self.conversations.dirty = false,
                Err(error) => self.error = Some(format!("Draft not saved: {error}")),
            }
        }
    }

    pub(crate) fn persist_conversation(&mut self) {
        let key = self.conversation_id.clone().unwrap_or_default();
        let mut position = Position {
            draft: self.input.current_value().to_owned(),
            scroll: self.chat_scroll.scroll_position().1,
            follow: self.follow_output,
        };
        if self.conversations.restoring_history {
            if let Some(previous) = self.conversations.positions.get(&key) {
                position.scroll = previous.scroll;
                position.follow = previous.follow;
            }
        }
        if self.conversations.positions.get(&key) != Some(&position) {
            self.conversations.positions.insert(key, position);
            self.conversations.dirty = true;
        }
        if self.conversations.active != self.conversation_id
            || self.conversations.run != self.current_run
            || self.conversations.unresolved != self.run_unresolved
        {
            self.conversations.active = self.conversation_id.clone();
            self.conversations.run = self.current_run.clone();
            self.conversations.unresolved = self.run_unresolved;
            self.conversations.dirty = true;
        }
    }

    fn restore_position(&mut self) {
        let key = self.conversation_id.as_deref().unwrap_or_default();
        let position = self
            .conversations
            .positions
            .get(key)
            .cloned()
            .unwrap_or(Position {
                follow: true,
                ..Position::default()
            });
        self.input.set_value(position.draft);
        self.follow_output = position.follow;
        self.unread_output = false;
        self.update_scroll_dimensions();
        self.chat_scroll.scroll_to(position.scroll);
        if self.follow_output {
            self.chat_scroll.scroll_to_bottom();
        }
    }

    fn can_switch(&self) -> bool {
        !self.run_unresolved
            && !self.waiting_response
            && !self.stopping
            && self.pending_approval.is_none()
            && !self.approval_in_flight
    }

    pub(crate) fn open_conversations(&mut self) -> Cmd<Msg> {
        self.persist_conversation();
        self.conversations.open = true;
        self.conversations.query.clear();
        self.conversations.selected = 0;
        self.conversations.error = None;
        let Some(connection) = self.connection.clone() else {
            self.conversations.error = Some("Connect to the daemon to load conversations.".into());
            return Cmd::none();
        };
        Cmd::spawn(async move {
            let result = async {
                let url = connection.endpoint(&["conversations"])?;
                connection
                    .client
                    .get(url)
                    .query(&[("limit", 200)])
                    .send()
                    .await
                    .map_err(|e| e.to_string())?
                    .error_for_status()
                    .map_err(|e| e.to_string())?
                    .json()
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            Msg::ConversationsListed(result)
        })
    }

    pub(crate) fn conversation_event(&mut self, event: &Event) -> Option<Msg> {
        let Event::Key(key) = event else { return None };
        match key.key {
            Key::Escape | Key::F(2) => {
                self.conversations.open = false;
                self.conversations.pending = None;
            }
            Key::Up => self.conversations.selected = self.conversations.selected.saturating_sub(1),
            Key::Down => {
                self.conversations.selected = (self.conversations.selected + 1)
                    .min(self.conversations.filtered().len().saturating_sub(1))
            }
            Key::Char('n') if key.modifiers.ctrl => {
                if self.can_switch() {
                    self.flush_drafts();
                    self.conversations.restoring_history = false;
                    self.conversation_id = None;
                    self.current_run = None;
                    self.current_response = None;
                    self.messages.clear();
                    self.tool_calls.clear();
                    self.tool_ids.clear();
                    self.streaming_content.clear();
                    self.session_id = None;
                    self.error = None;
                    self.status.step = "ready".into();
                    self.conversations.pending = None;
                    self.conversations.open = false;
                    self.restore_position();
                    self.flush_drafts();
                } else {
                    self.conversations.error =
                        Some("Resolve current work before switching conversations.".into());
                }
            }
            Key::Enter => {
                if !self.can_switch() {
                    self.conversations.error =
                        Some("Resolve current work before switching conversations.".into());
                    return None;
                }
                if let Some(row) = self
                    .conversations
                    .filtered()
                    .get(self.conversations.selected)
                {
                    let id = row.id.clone();
                    self.conversations.generation = self.conversations.generation.wrapping_add(1);
                    let generation = self.conversations.generation;
                    self.conversations.pending = Some((id.clone(), generation));
                    return Some(Msg::LoadConversation(id, generation));
                }
            }
            Key::Backspace => {
                self.conversations.query.pop();
                self.conversations.selected = 0;
            }
            Key::Char(character) if !key.modifiers.ctrl && !key.modifiers.alt => {
                self.conversations.query.push(character);
                self.conversations.selected = 0;
            }
            _ => {}
        }
        None
    }

    pub(crate) fn load_conversation(&self, id: String, generation: u64) -> Cmd<Msg> {
        let Some(connection) = self.connection.clone() else {
            return Cmd::none();
        };
        Cmd::spawn(async move {
            #[derive(Deserialize)]
            struct Message {
                role: String,
                content: String,
                created_at: String,
            }
            #[derive(Deserialize)]
            struct Detail {
                id: String,
                messages: Vec<Message>,
            }
            let result = async {
                let url = connection.endpoint(&["conversations", &id])?;
                let detail: Detail = connection
                    .client
                    .get(url)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?
                    .error_for_status()
                    .map_err(|e| e.to_string())?
                    .json()
                    .await
                    .map_err(|e| e.to_string())?;
                if detail.id != id {
                    return Err(
                        "Daemon returned a different conversation; nothing switched.".into(),
                    );
                }
                Ok(detail
                    .messages
                    .into_iter()
                    .map(|message| ChatMessage {
                        role: message.role,
                        content: message.content,
                        timestamp: message.created_at,
                    })
                    .collect())
            }
            .await;
            Msg::ConversationLoaded(id, generation, result)
        })
    }

    pub(crate) fn conversation_loaded(
        &mut self,
        id: String,
        generation: u64,
        result: Result<Vec<ChatMessage>, String>,
    ) -> Cmd<Msg> {
        if self.conversations.pending.as_ref() != Some(&(id.clone(), generation))
            || !self.conversations.open
        {
            return Cmd::none();
        }
        self.conversations.pending = None;
        match result {
            Ok(messages) if self.can_switch() => {
                self.flush_drafts();
                self.conversations.restoring_history = false;
                self.messages = messages;
                self.conversation_id = Some(id);
                self.current_run = None;
                self.current_response = None;
                self.tool_calls.clear();
                self.tool_ids.clear();
                self.streaming_content.clear();
                self.session_id = None;
                self.error = None;
                self.status.step = "ready".into();
                self.conversations.open = false;
                self.restore_position();
                self.flush_drafts();
                Cmd::none()
            }
            Ok(_) => Cmd::ShowToast(Toast::warning(
                "Resolve current work before switching conversations.",
            )),
            Err(error) => {
                self.conversations.error = Some(error);
                Cmd::none()
            }
        }
    }

    pub(crate) fn conversation_view(&self) -> Column {
        let mut view = Column::new()
            .padding(1.0)
            .child(Text::new(
                "Conversations · Esc back · Ctrl+N new · Enter open",
            ))
            .child(Text::new(format!("Search: {}", self.conversations.query)));
        if let Some(error) = &self.conversations.error {
            view = view.child(Text::new(error.clone()));
        }
        if self.conversations.pending.is_some() {
            view = view.child(Text::new("Loading conversation…"));
        }
        let rows = self.conversations.filtered();
        if rows.is_empty() {
            view = view.child(Text::new(
                "No matching conversations. Ctrl+N starts a new conversation.",
            ));
        }
        let start = self.conversations.selected.saturating_sub(5);
        for (index, row) in rows
            .iter()
            .enumerate()
            .skip(start)
            .take(self.terminal_height.saturating_sub(6) as usize)
        {
            view = view.child(Text::new(format!(
                "{} {} · {} messages · {}",
                if index == self.conversations.selected {
                    ">"
                } else {
                    " "
                },
                row.title.as_deref().unwrap_or("Untitled conversation"),
                row.message_count,
                row.updated_at
            )));
        }
        view
    }
}

impl Drop for Conversations {
    fn drop(&mut self) {
        if let Some(lock) = &self.lock {
            let _ = fs::remove_file(lock);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulttui::event::{KeyEvent, Modifiers};

    fn key(key: Key, ctrl: bool) -> Event {
        Event::Key(KeyEvent {
            key,
            modifiers: Modifiers {
                ctrl,
                ..Modifiers::default()
            },
            focused_id: Some("chat-input".into()),
        })
    }

    fn select(app: &mut PorygonTui, id: &str) {
        app.conversations.open = true;
        app.conversations.pending = Some((id.into(), 0));
        app.conversation_loaded(
            id.into(),
            0,
            Ok(vec![ChatMessage {
                role: "user".into(),
                content: format!("history {id}"),
                timestamp: "now".into(),
            }]),
        );
    }

    #[test]
    fn test_switch_restores_each_draft_and_reading_position() {
        let mut app = PorygonTui::new();
        app.input.set_value("new draft\nsecond line");
        select(&mut app, "a");
        app.input.set_value("draft a");
        app.follow_output = false;
        select(&mut app, "b");
        app.input.set_value("draft b");
        select(&mut app, "a");
        assert_eq!(app.input.current_value(), "draft a");
        assert!(!app.follow_output);
        app.conversations.open = true;
        app.conversation_event(&key(Key::Char('n'), true));
        assert_eq!(app.input.current_value(), "new draft\nsecond line");
        assert_eq!(app.conversation_id, None);
    }

    #[test]
    fn test_picker_enter_never_submits_or_consumes_draft() {
        let mut app = PorygonTui::new();
        app.input.set_value("do not send");
        app.conversations.open = true;
        app.conversations.rows.push(Conversation {
            id: "a".into(),
            title: Some("Research".into()),
            message_count: 3,
            updated_at: "today".into(),
        });
        assert!(
            matches!(app.handle_event(&key(Key::Enter, false)), Some(Msg::LoadConversation(id, _)) if id == "a")
        );
        assert_eq!(app.input.current_value(), "do not send");
        assert!(!app.waiting_response);
        assert!(app.current_run.is_none());
    }

    #[test]
    fn test_busy_and_late_load_cannot_change_turn_identity() {
        let mut app = PorygonTui::new();
        app.conversation_id = Some("active".into());
        app.current_run = Some("run".into());
        app.run_unresolved = true;
        app.input.set_value("preserved");
        select(&mut app, "other");
        app.conversation_event(&key(Key::Char('n'), true));
        assert_eq!(app.conversation_id.as_deref(), Some("active"));
        assert_eq!(app.current_run.as_deref(), Some("run"));
        assert_eq!(app.input.current_value(), "preserved");
        app.run_unresolved = false;
        app.conversations.pending = Some(("other".into(), 0));
        app.conversation_event(&key(Key::Escape, false));
        app.conversation_loaded("other".into(), 0, Ok(Vec::new()));
        assert_eq!(app.conversation_id.as_deref(), Some("active"));
    }

    #[test]
    fn test_persistent_store_roundtrip_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("porygon-drafts-test-{}", std::process::id()));
        let path = directory.join("drafts.json");
        let mut store = Conversations::default();
        store.path = Some(path.clone());
        store.positions.insert(
            "conversation".into(),
            Position {
                draft: "exact\ntext ✓".into(),
                scroll: 42.0,
                follow: false,
            },
        );
        store.write().unwrap();
        let restored: DraftStore = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(restored.positions["conversation"].draft, "exact\ntext ✓");
        assert_eq!(restored.positions["conversation"].scroll, 42.0);
        assert!(!restored.positions["conversation"].follow);
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn test_quit_retains_stop_safety_with_picker_open() {
        let mut app = PorygonTui::new();
        app.conversations.open = true;
        app.waiting_response = true;
        assert!(matches!(
            app.handle_event(&Event::Quit),
            Some(Msg::AbortStream)
        ));
        app.waiting_response = false;
        assert!(matches!(
            app.handle_event(&Event::Quit),
            Some(Msg::ExitRequested)
        ));
    }

    #[test]
    fn test_typing_caches_without_disk_io_until_flush() {
        let directory =
            std::env::temp_dir().join(format!("porygon-drafts-cache-{}", std::process::id()));
        let path = directory.join("drafts.json");
        let mut app = PorygonTui::new();
        app.conversations.path = Some(path.clone());
        for index in 0..1000 {
            app.input.set_value(format!("draft {index}"));
            app.persist_conversation();
        }
        assert!(!path.exists(), "key edits must not touch the filesystem");
        app.flush_drafts();
        let store: DraftStore = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(store.positions[""].draft, "draft 999");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn test_restart_restores_active_draft_and_uncertainty_without_submission() {
        let mut app = PorygonTui::new();
        let mut store = DraftStore {
            active: Some("active".into()),
            run: Some("unconfirmed".into()),
            unresolved: true,
            ..DraftStore::default()
        };
        store.positions.insert(
            "active".into(),
            Position {
                draft: "next thought".into(),
                scroll: 123.0,
                follow: false,
            },
        );
        app.restore_store(store);
        app.restore_position();
        app.persist_conversation();
        assert_eq!(app.input.current_value(), "next thought");
        assert_eq!(app.conversation_id.as_deref(), Some("active"));
        assert_eq!(app.current_run.as_deref(), Some("unconfirmed"));
        assert_eq!(app.conversations.positions["active"].scroll, 123.0);
        assert!(!app.waiting_response);
        assert!(app.run_unresolved);
        assert!(!app.run_can_continue);
    }

    #[test]
    fn test_assigned_conversation_takes_ownership_of_next_draft() {
        let mut app = PorygonTui::new();
        app.input.set_value("submitted text");
        app.persist_conversation();
        app.input.set_value("next thought");
        app.assign_conversation("created".into());
        assert!(!app.conversations.positions.contains_key(""));
        assert_eq!(app.conversations.positions["created"].draft, "next thought");
        app.conversations.open = true;
        app.conversation_event(&key(Key::Char('n'), true));
        assert!(app.input.current_value().is_empty());
    }

    #[test]
    fn test_previous_same_id_load_generation_is_ignored() {
        let mut app = PorygonTui::new();
        app.conversations.open = true;
        app.conversations.pending = Some(("same".into(), 2));
        app.conversation_loaded("same".into(), 1, Ok(Vec::new()));
        assert!(app.conversation_id.is_none());
        assert_eq!(app.conversations.pending, Some(("same".into(), 2)));
    }
}
