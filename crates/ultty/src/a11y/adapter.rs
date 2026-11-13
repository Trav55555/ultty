//! The platform accessibility adapter for Ultty's window, and when it is
//! sent a new tree.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use accesskit_winit::{Event, WindowEvent as AccessKitEvent};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::Window;

use super::live::LiveRegions;
use super::scene::NativeActionTarget;
use super::tree::{self, region_node_id, Snapshot};

/// The shortest time between two trees sent to assistive technology
/// (ADR 0021 §3): output that changes the screen faster is coalesced.
pub const REBUILD_INTERVAL: Duration = Duration::from_millis(50);

/// Owns the AccessKit adapter for one window.
///
/// Until a screen reader asks for the tree it costs a match per window
/// event and, on Linux, the D-Bus client's threads: the adapter watches the
/// session bus from its own thread and joins the AT-SPI bus only while
/// assistive technology is enabled. Without a session bus it stays inert.
pub struct Adapter {
    inner: accesskit_winit::Adapter,
    /// The window's title as last set. winit cannot read it back on X11 or
    /// Wayland (`Window::title` is always empty there).
    window_title: String,
    rebuilds: Rebuilds,
    /// What [`Adapter::observe_shown`] last saw.
    shown: Option<u64>,
    /// Live regions' queues and announcements.
    live: LiveRegions,
    /// Reverse map of only the native nodes in the last emitted tree.
    actions: HashMap<accesskit::NodeId, NativeActionTarget>,
}

impl Adapter {
    /// Create the adapter for `window`, titled `window_title`, which must
    /// not have been shown yet.
    ///
    /// AccessKit's requests arrive on the event loop through `proxy` and go
    /// to [`Adapter::handle`].
    ///
    /// # Panics
    ///
    /// If `window` is already visible: AccessKit must see the window before
    /// it is first shown.
    pub fn new<T: From<Event> + Send + 'static>(
        event_loop: &ActiveEventLoop,
        window: &Window,
        window_title: &str,
        proxy: EventLoopProxy<T>,
    ) -> Self {
        Self {
            inner: accesskit_winit::Adapter::with_event_loop_proxy(event_loop, window, proxy),
            window_title: window_title.to_owned(),
            rebuilds: Rebuilds::default(),
            shown: None,
            live: LiveRegions::default(),
            actions: HashMap::new(),
        }
    }

    /// Let the adapter see a window event (bounds and focus) before the
    /// app handles it.
    pub fn process_event(&mut self, window: &Window, event: &WindowEvent) {
        self.inner.process_event(window, event);
    }

    /// Answer a request AccessKit sent through the event loop. A tree it
    /// asked for is sent by the next [`Adapter::update`].
    pub fn handle(
        &mut self,
        window: &Window,
        event: AccessKitEvent,
    ) -> Option<(NativeActionTarget, ultty_protocol::SemanticActionKind)> {
        match event {
            AccessKitEvent::InitialTreeRequested => {
                tracing::debug!("Accessibility activated");
                // The adapter learns the window's bounds only from Moved and
                // Resized events; one that was never moved or resized would
                // be a 0x0 frame to screen readers and magnifiers.
                self.inner
                    .process_event(window, &WindowEvent::Resized(window.inner_size()));
                self.rebuilds.activate();
                self.shown = None;
                self.live.clear();
                self.actions.clear();
            }
            AccessKitEvent::AccessibilityDeactivated => {
                // Nothing is built or sent until the next
                // `InitialTreeRequested`.
                tracing::debug!("Accessibility deactivated");
                self.rebuilds.deactivate();
                self.live.clear();
                self.actions.clear();
            }
            AccessKitEvent::ActionRequested(request) => {
                let action = match request.action {
                    accesskit::Action::Focus => ultty_protocol::SemanticActionKind::Focus,
                    accesskit::Action::Click => ultty_protocol::SemanticActionKind::Activate,
                    _ => return None,
                };
                let target = self.actions.get(&request.target_node)?;
                return (target.actions & action.mask() != 0).then(|| (target.clone(), action));
            }
        }
        None
    }

    /// Whether assistive technology is listening. While it is not, the app
    /// gathers nothing for the tree.
    pub fn is_active(&self) -> bool {
        self.rebuilds.active
    }

    /// The window was retitled.
    pub fn set_window_title(&mut self, title: &str) {
        if self.window_title != title {
            self.window_title = title.to_owned();
            self.rebuilds.changed();
        }
    }

    /// Something the tree shows changed: the focused pane's screen or a
    /// title. Ignored while inactive.
    pub fn changed(&mut self) {
        self.rebuilds.changed();
    }

    /// Note a digest of the window's structure (tabs, panes, focus,
    /// geometry); a new value means the tree must be rebuilt.
    pub fn observe_shown(&mut self, digest: u64) {
        if self.shown != Some(digest) {
            self.shown = Some(digest);
            self.rebuilds.changed();
        }
    }

    /// `text` was appended at `now` to `region` of native `session`, whose
    /// `live` is as given; it is announced when its flush is due
    /// ([`super::live`]). Ignored while inactive.
    pub fn appended(
        &mut self,
        session: u64,
        region: u32,
        live: ultty_protocol::Live,
        text: &str,
        now: Instant,
    ) {
        if self.rebuilds.active {
            self.live.push(session, region, live, text, now);
        }
    }

    /// `region` of native `session` was deleted; whatever it queued or
    /// announced goes with it.
    pub fn removed(&mut self, session: u64, region: u32) {
        self.live.forget(session, region);
    }

    /// When the loop must wake to send a coalesced tree or flush a live
    /// region, if either is waiting. `None` while inactive or with nothing
    /// changed or queued.
    pub fn next_deadline(&self) -> Option<Instant> {
        match (self.rebuilds.deadline(), self.live.next_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Flush the live regions that are due at `now`, then send a tree of
    /// what `snapshot` gathers if one is due; `snapshot` is not called
    /// otherwise.
    pub fn update(&mut self, now: Instant, snapshot: impl FnOnce() -> Snapshot) {
        if self.live.flush(now) {
            self.rebuilds.changed();
        }
        if !self.rebuilds.take_due(now) {
            return;
        }
        let title = &self.window_title;
        let live = &mut self.live;
        let actions = &mut self.actions;
        self.inner.update_if_active(|| {
            let started = Instant::now();
            let mut snapshot = snapshot();
            if !live.is_empty() {
                snapshot.set_announcements(live);
            }
            let update = tree::build(title, &snapshot);
            *actions = published_actions(&snapshot, &update);
            if !live.is_empty() {
                let built: std::collections::HashSet<accesskit::NodeId> =
                    update.nodes.iter().map(|(id, _)| *id).collect();
                live.retain(|session, region| built.contains(&region_node_id(session, region)));
            }
            tracing::debug!(
                target: "ultty::a11y",
                nodes = update.nodes.len(),
                micros = started.elapsed().as_micros(),
                "accessibility tree built"
            );
            update
        });
    }
}

/// Build the reverse map from nodes actually emitted, never by decoding or
/// guessing a hashed node ID. Omitted, disabled and colliding nodes cannot act.
fn published_actions(
    snapshot: &Snapshot,
    update: &accesskit::TreeUpdate,
) -> HashMap<accesskit::NodeId, NativeActionTarget> {
    let emitted: HashMap<_, _> = update.nodes.iter().map(|(id, node)| (*id, node)).collect();
    let mut result = HashMap::new();
    let mut collisions = std::collections::HashSet::new();
    for pane in &snapshot.panes {
        let surfaces = pane.surfaces.iter().map(|surface| &surface.scene);
        for target in pane
            .scenes
            .iter()
            .chain(surfaces)
            .flat_map(|scene| scene.action_targets())
        {
            let id = region_node_id(target.session, target.region);
            let Some(node) = emitted.get(&id) else {
                continue;
            };
            if !node.supports_action(accesskit::Action::Focus)
                && !node.supports_action(accesskit::Action::Click)
            {
                continue;
            }
            if result.insert(id, target.clone()).is_some() {
                collisions.insert(id);
            }
        }
    }
    for id in collisions {
        result.remove(&id);
    }
    result
}

/// Whether and when to rebuild the tree: only while assistive technology
/// is active, only after a change, and at most once per
/// [`REBUILD_INTERVAL`]. The first change after a quiet interval is sent at
/// once, so a keystroke's echo is not delayed; changes inside the interval
/// wait for its end and go out as one tree.
#[derive(Debug, Default)]
struct Rebuilds {
    active: bool,
    /// A change not yet sent.
    pending: bool,
    last_sent: Option<Instant>,
}

impl Rebuilds {
    /// Assistive technology asked for the tree: send one at once.
    fn activate(&mut self) {
        *self = Self {
            active: true,
            pending: true,
            last_sent: None,
        };
    }

    fn deactivate(&mut self) {
        *self = Self::default();
    }

    fn changed(&mut self) {
        self.pending |= self.active;
    }

    fn deadline(&self) -> Option<Instant> {
        if !self.pending {
            return None;
        }
        // `pending` is only ever set while active.
        Some(
            self.last_sent
                .map_or_else(Instant::now, |sent| sent + REBUILD_INTERVAL),
        )
    }

    /// Whether to rebuild at `now`; if so, the change counts as sent.
    fn take_due(&mut self, now: Instant) -> bool {
        let due = self.pending
            && self
                .last_sent
                .is_none_or(|sent| now.saturating_duration_since(sent) >= REBUILD_INTERVAL);
        if due {
            self.pending = false;
            self.last_sent = Some(now);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn activation_sends_a_tree_at_once() {
        let mut rebuilds = Rebuilds::default();
        rebuilds.activate();
        let now = Instant::now();
        assert!(rebuilds.deadline().is_some_and(|at| at <= Instant::now()));
        assert!(rebuilds.take_due(now));
        assert!(!rebuilds.take_due(now));
        assert_eq!(rebuilds.deadline(), None);
    }

    #[test]
    fn changes_within_the_interval_coalesce_into_one_rebuild() {
        let mut rebuilds = Rebuilds::default();
        rebuilds.activate();
        let start = Instant::now();
        assert!(rebuilds.take_due(start));

        let mut sent = 0;
        for step in 1..=40 {
            rebuilds.changed();
            sent += usize::from(rebuilds.take_due(start + ms(step)));
        }
        assert_eq!(sent, 0, "nothing goes out inside the interval");
        assert_eq!(rebuilds.deadline(), Some(start + REBUILD_INTERVAL));
        assert!(rebuilds.take_due(start + REBUILD_INTERVAL));
        assert!(!rebuilds.take_due(start + ms(80)));
        assert_eq!(rebuilds.deadline(), None);
    }

    #[test]
    fn a_change_after_a_quiet_interval_goes_out_at_once() {
        let mut rebuilds = Rebuilds::default();
        rebuilds.activate();
        let start = Instant::now();
        assert!(rebuilds.take_due(start));
        rebuilds.changed();
        assert!(rebuilds.take_due(start + ms(500)));
    }

    #[test]
    fn nothing_is_rebuilt_or_scheduled_while_inactive() {
        let mut rebuilds = Rebuilds::default();
        let now = Instant::now();
        rebuilds.changed();
        assert_eq!(rebuilds.deadline(), None);
        assert!(!rebuilds.take_due(now));

        rebuilds.activate();
        assert!(rebuilds.take_due(now));
        rebuilds.deactivate();
        rebuilds.changed();
        assert_eq!(rebuilds.deadline(), None);
        assert!(!rebuilds.take_due(now + ms(100)));
    }

    #[test]
    fn a_still_screen_schedules_nothing_while_active() {
        let mut rebuilds = Rebuilds::default();
        rebuilds.activate();
        let now = Instant::now();
        assert!(rebuilds.take_due(now));
        assert_eq!(rebuilds.deadline(), None);
        assert!(!rebuilds.take_due(now + ms(1000)));
    }
}
