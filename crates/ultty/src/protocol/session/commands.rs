//! Native protocol command application.

use std::io;
use std::time::Instant;

use tracing::{debug, info, warn};
use ultty_protocol::{
    negotiate_capabilities, negotiate_protocol_version, AppMessage, Constraints, Content, RegionId,
    Semantics, Style, SurfaceEventKind, TerminalMessage, CAP_APPEND_CONTENT, CAP_INLINE_SURFACES,
    CAP_REGION_METADATA, CAP_SEMANTICS, CAP_STREAM_LIFECYCLE, PROTOCOL_VERSION,
};

use super::content_limits::{text_bytes, trim_region_text, MAX_SESSION_TEXT_BYTES};
use super::errors::NativeProtocolError;
use super::{NativeSession, StyleAnimation};
use crate::scene::region::Region;

/// Whether applying `msg` may change what the accessibility tree shows of
/// the session, its focus included. Keepalives, the clipboard,
/// and subscriptions do not. Styles count because they can hide a target.
/// Streams also count: an open one makes its
/// region busy (ADR 0021 §3).
fn changes_what_is_shown(msg: &AppMessage) -> bool {
    !matches!(
        msg,
        AppMessage::ClientHello { .. }
            | AppMessage::Subscribe { .. }
            | AppMessage::Unsubscribe { .. }
            | AppMessage::ClipboardGet
            | AppMessage::ClipboardSet { .. }
            | AppMessage::Pong
            | AppMessage::OverrideResult { .. }
            | AppMessage::ActionResult { .. }
            | AppMessage::Disconnect
    )
}

impl NativeSession {
    /// Apply one decoded app message outside frame batching. A message
    /// that may change what is shown, and was not refused, counts as a
    /// change in [`NativeSession::shown_generation`].
    pub(super) fn apply_message(&mut self, msg: AppMessage) -> io::Result<bool> {
        let shown = changes_what_is_shown(&msg);
        self.note_surfaces_touched(&msg);
        let errors = self.protocol_errors;
        let result = self.apply_message_inner(msg);
        if shown && self.protocol_errors == errors {
            self.mark_shown_changed();
        }
        result
    }

    fn apply_message_inner(&mut self, msg: AppMessage) -> io::Result<bool> {
        match msg {
            AppMessage::ClientHello {
                version,
                requested_capabilities,
            } => {
                self.handle_client_hello(version, requested_capabilities)?;
            }

            AppMessage::CreateRegion {
                id,
                parent,
                insert_index,
                constraints,
                style,
            } => {
                self.handle_create_region(id, parent, insert_index, constraints, style);
            }

            AppMessage::SetConstraints { id, constraints } => {
                self.handle_set_constraints(id, constraints);
            }

            AppMessage::SetStyle { id, style } => {
                self.handle_set_style(id, style);
            }

            AppMessage::SetContent { id, content } => {
                self.handle_set_content(id, content);
            }

            AppMessage::SetRegionMetadata { id, metadata } => {
                if self.has_capability(CAP_REGION_METADATA) {
                    self.handle_set_region_metadata(id, metadata);
                } else {
                    self.queue_protocol_error(NativeProtocolError::missing_capability(
                        "set_region_metadata",
                    ));
                }
            }

            AppMessage::SetSemantics { id, semantics } => {
                if self.has_capability(CAP_SEMANTICS) {
                    self.handle_set_semantics(id, semantics);
                } else {
                    self.queue_protocol_error(NativeProtocolError::missing_capability(
                        "set_semantics",
                    ));
                }
            }

            AppMessage::AppendContent { id, text } => {
                if self.has_capability(CAP_APPEND_CONTENT) {
                    self.handle_append_content(id, text);
                } else {
                    self.queue_protocol_error(NativeProtocolError::missing_capability(
                        "append_content",
                    ));
                }
            }

            AppMessage::StreamStart { id, stream_id } => {
                if self.has_capability(CAP_STREAM_LIFECYCLE) {
                    debug!("Stream {} started on region {:?}", stream_id, id);
                    self.active_streams.insert(id, stream_id);
                    if let Some(region) = self.scene.get_mut(id) {
                        region.streaming = true;
                    }
                } else {
                    self.queue_protocol_error(NativeProtocolError::missing_capability(
                        "stream_start",
                    ));
                }
            }

            AppMessage::StreamEnd { id, stream_id } => {
                if self.has_capability(CAP_STREAM_LIFECYCLE) {
                    debug!("Stream {} ended on region {:?}", stream_id, id);
                    if let Some(active_id) = self.active_streams.get(&id) {
                        if *active_id == stream_id {
                            self.active_streams.remove(&id);
                            if let Some(region) = self.scene.get_mut(id) {
                                region.streaming = false;
                            }
                        } else {
                            warn!(
                                "StreamEnd mismatch for {:?}: active={}, got={}",
                                id, active_id, stream_id
                            );
                        }
                    }
                } else {
                    self.queue_protocol_error(NativeProtocolError::missing_capability(
                        "stream_end",
                    ));
                }
            }

            AppMessage::DeleteRegion { id } => {
                self.handle_delete_region(id)?;
            }

            AppMessage::CreateSurface {
                id,
                rows,
                constraints,
                style,
            } => {
                if self.has_capability(CAP_INLINE_SURFACES) {
                    self.handle_create_surface(id, rows, constraints, style);
                } else {
                    self.queue_protocol_error(NativeProtocolError::missing_capability(
                        "create_surface",
                    ));
                }
            }

            AppMessage::CloseSurface { id } => {
                if self.has_capability(CAP_INLINE_SURFACES) {
                    self.handle_close_surface(id)?;
                } else {
                    self.queue_protocol_error(NativeProtocolError::missing_capability(
                        "close_surface",
                    ));
                }
            }

            AppMessage::ReparentRegion {
                id,
                new_parent,
                insert_index,
            } => {
                self.handle_reparent_region(id, new_parent, insert_index);
            }

            AppMessage::BeginFrame { .. } | AppMessage::EndFrame { .. } => {
                self.queue_protocol_error(NativeProtocolError::bad_request(
                    "frame control message reached command application",
                ));
            }

            AppMessage::Subscribe { id, events } => {
                if events & ultty_protocol::EVENT_KEY != 0 && self.scene.is_in_surface(id) {
                    // An inline-only app reads keys from its tty (ADR 0020).
                    warn!("Subscribe to key events on surface region {:?} refused", id);
                    self.queue_protocol_error(NativeProtocolError::bad_request(format!(
                        "subscribe: region {} is in a surface, which gets no key events",
                        id.0
                    )));
                } else {
                    self.subscriptions.insert(id, events);
                }
            }

            AppMessage::Unsubscribe { id, events } => {
                if let Some(mask) = self.subscriptions.get_mut(&id) {
                    *mask &= !events;
                    if *mask == 0 {
                        self.subscriptions.remove(&id);
                    }
                }
            }

            AppMessage::Scroll { id, x, y, animate } => {
                if animate {
                    self.scroll_targets.insert(id, (x, y));
                    self.scroll_offsets.entry(id).or_insert((0.0, 0.0));
                } else {
                    self.scroll_offsets.insert(id, (x, y));
                    self.scroll_targets.remove(&id);
                    if self.is_subscribed(id, ultty_protocol::EVENT_SCROLL) {
                        let _ = self.send_scroll_event(id, x, y);
                    }
                }
                self.mark_scroll_position_changed();
                debug!(
                    "Scroll set for {:?}: ({}, {}), animate={}",
                    id, x, y, animate
                );
            }

            AppMessage::Focus { id } => {
                let old = self.focused_region.take();
                self.focused_region = Some(id);
                debug!("Focus set to {:?} (was {:?})", id, old);

                if let Some(old_id) = old {
                    if old_id != id && self.is_subscribed(old_id, ultty_protocol::EVENT_FOCUS) {
                        let _ = self.send_message(&TerminalMessage::Focus(
                            ultty_protocol::FocusEvent {
                                region_id: old_id,
                                focused: false,
                            },
                        ));
                    }
                }

                if self.is_subscribed(id, ultty_protocol::EVENT_FOCUS) {
                    let _ =
                        self.send_message(&TerminalMessage::Focus(ultty_protocol::FocusEvent {
                            region_id: id,
                            focused: true,
                        }));
                }
            }

            AppMessage::Blur => {
                if let Some(old_id) = self.focused_region.take() {
                    debug!("Blur from {:?}", old_id);
                    if self.is_subscribed(old_id, ultty_protocol::EVENT_FOCUS) {
                        let _ = self.send_message(&TerminalMessage::Focus(
                            ultty_protocol::FocusEvent {
                                region_id: old_id,
                                focused: false,
                            },
                        ));
                    }
                } else {
                    debug!("Blur (no focus)");
                }
            }

            AppMessage::ClipboardGet => {
                debug!("Session {} clipboard get request", self.session_id);
                self.pending_clipboard_get = true;
            }

            AppMessage::ClipboardSet { text } => {
                debug!(
                    "Session {} clipboard set: {} chars",
                    self.session_id,
                    text.len()
                );
                self.pending_clipboard_set = Some(text);
            }

            AppMessage::Pong => {
                debug!("Session {} received pong", self.session_id);
                self.awaiting_pong = false;
            }

            AppMessage::OverrideResult { request_id, error } => {
                debug!(
                    "Session {} answered override push {}: {}",
                    self.session_id,
                    request_id,
                    error.as_deref().unwrap_or("applied")
                );
                self.accept_override_result(request_id, error);
            }

            AppMessage::ActionResult { request_id, error } => {
                if self.has_capability(ultty_protocol::CAP_SEMANTIC_ACTIONS) {
                    self.accept_semantic_result(request_id, error);
                } else {
                    self.queue_protocol_error(NativeProtocolError::missing_capability(
                        "action_result",
                    ));
                }
            }

            AppMessage::Disconnect => {
                info!("Session {} disconnected (explicit)", self.session_id);
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Handle ClientHello - send ServerHello and SessionReady.
    /// Answer `ClientHello`. In-band, the answer goes out at once and what
    /// follows is held until the app confirms.
    fn handle_client_hello(&mut self, version: u16, requested: u32) -> io::Result<()> {
        self.answer_client_hello(version, requested)?;
        self.end_in_band_hello()
    }

    fn answer_client_hello(&mut self, version: u16, requested: u32) -> io::Result<()> {
        info!(
            "Session {} client hello: version={}, caps=0x{:04x}",
            self.session_id, version, requested
        );

        let Some(negotiated_version) = negotiate_protocol_version(version, PROTOCOL_VERSION) else {
            self.queue_protocol_error(NativeProtocolError::unsupported_version(version));
            return Ok(());
        };

        self.protocol_version = negotiated_version;
        let capabilities = negotiate_capabilities(requested, self.supported_capabilities());
        if self.capabilities != capabilities {
            // A newly negotiated action capability requires a fresh committed frame.
            self.semantic_committed_frame = None;
        }
        self.capabilities = capabilities;

        let hello = TerminalMessage::ServerHello {
            version: negotiated_version,
            capabilities: self.capabilities,
        };
        self.send_message(&hello)?;

        let ready = TerminalMessage::SessionReady {
            session_id: self.session_id,
            root_region: RegionId::ROOT,
            width: self.viewport_width as u16,
            height: self.viewport_height as u16,
        };
        self.send_message(&ready)?;

        Ok(())
    }

    pub(super) fn has_capability(&self, capability: u32) -> bool {
        self.capabilities & capability != 0
    }

    /// Handle CreateRegion.
    fn handle_create_region(
        &mut self,
        id: RegionId,
        parent: RegionId,
        insert_index: i32,
        constraints: Constraints,
        style: Style,
    ) {
        let mut region = Region::new(id);
        region.parent = Some(parent);
        region.constraints = constraints;
        region.style = style;
        // A stream may be opened before its region is created.
        region.streaming = self.active_streams.contains_key(&id);

        let index = usize::try_from(insert_index).unwrap_or(usize::MAX);
        match self.scene.try_insert_at(region, index) {
            Ok(()) => {
                self.mark_full_scene_rebuild();
                debug!("Created region {:?} under {:?}", id, parent);
            }
            Err(error) => {
                warn!(
                    "CreateRegion {:?} under {:?} rejected: {}",
                    id, parent, error
                );
                self.queue_protocol_error(NativeProtocolError::scene_edit_rejected(
                    "create_region",
                    error,
                ));
            }
        }
    }

    /// Handle SetConstraints.
    fn handle_set_constraints(&mut self, id: RegionId, constraints: Constraints) {
        if let Some(region) = self.scene.get_mut(id) {
            region.constraints = constraints;
            region.mark_dirty();
            self.mark_region_constraints_changed(id);
        } else {
            warn!("SetConstraints for unknown region {:?}", id);
            self.queue_protocol_error(NativeProtocolError::region_not_found(id));
        }
    }

    /// Handle SetStyle.
    fn handle_set_style(&mut self, id: RegionId, style: Style) {
        if let Some(region) = self.scene.get_mut(id) {
            if let Some(transition) = &style.transition {
                let old_style = &region.style;
                let start_opacity = old_style.opacity;
                let start_bg = Self::style_color_to_rgb(&old_style.bg);
                let start_fg = Self::style_color_to_rgb(&old_style.fg);

                let anim = StyleAnimation {
                    start: std::time::Instant::now(),
                    duration: std::time::Duration::from_millis(transition.duration_ms as u64),
                    easing: transition.easing,
                    properties: transition.properties,
                    start_opacity,
                    start_bg,
                    start_fg,
                    target_style: style,
                };
                self.style_animations.insert(id, anim);
            } else {
                self.style_animations.remove(&id);
                region.style = style;
            }
            region.mark_dirty();
            self.mark_region_visual_changed(id);
        } else {
            warn!("SetStyle for unknown region {:?}", id);
            self.queue_protocol_error(NativeProtocolError::region_not_found(id));
        }
    }

    /// Extract RGB values from an optional Color, if it's an RGB color.
    fn style_color_to_rgb(color: &Option<ultty_protocol::Color>) -> Option<(u8, u8, u8)> {
        match color {
            Some(ultty_protocol::Color::Rgb(r, g, b)) => Some((*r, *g, *b)),
            _ => None,
        }
    }

    /// Handle SetContent.
    fn handle_set_content(&mut self, id: RegionId, content: Content) {
        if self.scene.get(id).is_some() && !self.admit_text(text_bytes(&content)) {
            self.refuse_text_over_limit(id);
            return;
        }
        if let Some(region) = self.scene.get_mut(id) {
            region.content = content;
            region.mark_dirty();
            self.mark_region_visual_changed(id);
        } else {
            warn!("SetContent for unknown region {:?}", id);
            self.queue_protocol_error(NativeProtocolError::region_not_found(id));
        }
    }

    /// Handle AppendContent - append text to existing region content.
    ///
    /// The region keeps only its newest text past a limit, like scrollback,
    /// and the session as a whole has a ceiling (see `content_limits`).
    fn handle_append_content(&mut self, id: RegionId, text: String) {
        if self.scene.get(id).is_some() && !self.admit_text(text.len()) {
            self.refuse_text_over_limit(id);
            return;
        }
        // Kept for a screen reader, if one listens and the region is live.
        let live = self.live_appends.is_some()
            && self
                .scene
                .get(id)
                .is_some_and(|region| region.semantics().live != ultty_protocol::Live::Off);
        let mut announce = live.then(|| text.clone());
        if let Some(region) = self.scene.get_mut(id) {
            match &mut region.content {
                Content::Text(tc) => {
                    tc.text.push_str(&text);
                }
                Content::RichText(rich) => {
                    if let Some(last) = rich.spans.last_mut() {
                        last.text.push_str(&text);
                    } else {
                        rich.spans.push(ultty_protocol::Span {
                            text,
                            style: ultty_protocol::Style::new(),
                        });
                    }
                }
                Content::Widget(ultty_protocol::WidgetContent::Markdown(md)) => {
                    md.source.push_str(&text);
                }
                Content::None => {
                    region.content = Content::Text(ultty_protocol::TextContent::new(text));
                }
                other => {
                    warn!(
                        "AppendContent on non-appendable content type for region {:?}: {:?}",
                        id,
                        std::mem::discriminant(other)
                    );
                    self.queue_protocol_error(NativeProtocolError::bad_request(format!(
                        "cannot append to content type of region {}",
                        id.0
                    )));
                    return;
                }
            }
            trim_region_text(&mut region.content);
            region.appended = true;
            region.mark_dirty();
            self.mark_region_visual_changed(id);
            if let Some(text) = announce.take() {
                self.note_live_append(id, &text);
            }
        } else {
            warn!("AppendContent for unknown region {:?}", id);
            self.queue_protocol_error(NativeProtocolError::region_not_found(id));
        }
    }

    fn refuse_text_over_limit(&mut self, id: RegionId) {
        warn!(
            "Session {} content for region {:?} refused: over {} bytes of text",
            self.session_id, id, MAX_SESSION_TEXT_BYTES
        );
        self.queue_protocol_error(NativeProtocolError::bad_request(format!(
            "session text would exceed {MAX_SESSION_TEXT_BYTES} bytes; content of region {} unchanged",
            id.0
        )));
    }

    /// Handle SetRegionMetadata.
    fn handle_set_region_metadata(&mut self, id: RegionId, metadata: Vec<(String, String)>) {
        if let Some(region) = self.scene.get_mut(id) {
            region.metadata = metadata;
        } else {
            warn!("SetRegionMetadata for unknown region {:?}", id);
            self.queue_protocol_error(NativeProtocolError::region_not_found(id));
        }
    }

    /// Handle SetSemantics. Semantics do not change what is drawn, so the
    /// scene stays clean.
    fn handle_set_semantics(&mut self, id: RegionId, semantics: Semantics) {
        if let Some(region) = self.scene.get_mut(id) {
            region.set_semantics(semantics);
        } else {
            warn!("SetSemantics for unknown region {:?}", id);
            self.queue_protocol_error(NativeProtocolError::region_not_found(id));
        }
    }

    /// Note which surfaces `msg` changes what is drawn of, before it is
    /// applied: a region created under one, or one it edits, scrolls,
    /// deletes or moves (from or to a surface). Deleting a surface root
    /// closes it, which is not a change of its scene.
    fn note_surfaces_touched(&mut self, msg: &AppMessage) {
        if self.surface_states.is_empty() {
            return;
        }
        let touched = match msg {
            AppMessage::CreateRegion { parent, .. } => [Some(*parent), None],
            AppMessage::DeleteRegion { id } if self.scene.surface(*id).is_some() => [None, None],
            AppMessage::SetConstraints { id, .. }
            | AppMessage::SetStyle { id, .. }
            | AppMessage::SetContent { id, .. }
            | AppMessage::AppendContent { id, .. }
            | AppMessage::DeleteRegion { id }
            | AppMessage::Scroll { id, .. } => [Some(*id), None],
            AppMessage::ReparentRegion { id, new_parent, .. } => [Some(*id), Some(*new_parent)],
            _ => [None, None],
        };
        for id in touched.into_iter().flatten() {
            self.note_surface_change(id);
        }
    }

    /// Handle DeleteRegion. On a surface root it closes the surface,
    /// keeping its last rows for its anchor's.
    fn handle_delete_region(&mut self, id: RegionId) -> io::Result<()> {
        let closes_surface = self.scene.surface(id).is_some();
        if closes_surface {
            self.freeze_surface(id);
        }
        if self.remove_tree(id) {
            if closes_surface {
                self.send_surface_event(id, SurfaceEventKind::Closed)?;
            }
        } else {
            warn!("DeleteRegion for unknown region {:?}", id);
            self.queue_protocol_error(NativeProtocolError::region_not_found(id));
        }
        Ok(())
    }

    /// Delete region `id` and its subtree, a surface's placement with its
    /// root, and the state of every region removed. False if there was no
    /// such region.
    pub(super) fn remove_tree(&mut self, id: RegionId) -> bool {
        let removed: Vec<RegionId> = self.scene.iter_tree(id).map(|region| region.id).collect();
        if self.scene.remove(id).is_none() {
            return false;
        }
        self.mark_full_scene_rebuild();
        self.mark_shown_changed();
        self.forget_regions(&removed);
        self.surface_states.remove(&id);
        debug!("Deleted region {:?}", id);
        true
    }

    /// Handle CreateSurface: a new root, laid out at the viewport's width by
    /// `rows`. The tree enforces the surface and region limits.
    fn handle_create_surface(
        &mut self,
        id: RegionId,
        rows: u16,
        constraints: Constraints,
        style: Style,
    ) {
        let mut region = Region::new(id);
        region.constraints = constraints;
        region.style = style;
        region.streaming = self.active_streams.contains_key(&id);
        match self.scene.try_insert_surface(region, rows) {
            Ok(()) => {
                self.mark_full_scene_rebuild();
                self.surface_states
                    .insert(id, super::surfaces::SurfaceState::new(Instant::now()));
                debug!("Created surface {:?} ({} rows)", id, rows);
            }
            Err(error) => {
                warn!("CreateSurface {:?} rejected: {}", id, error);
                self.queue_protocol_error(NativeProtocolError::scene_edit_rejected(
                    "create_surface",
                    error,
                ));
            }
        }
    }

    /// Handle CloseSurface: delete the surface's tree and say so.
    fn handle_close_surface(&mut self, id: RegionId) -> io::Result<()> {
        if self.scene.surface(id).is_some() {
            return self.handle_delete_region(id);
        }
        if self.scene.get(id).is_some() {
            warn!("CloseSurface for region {:?}, which is not a surface", id);
            self.queue_protocol_error(NativeProtocolError::bad_request(format!(
                "close_surface: region {} is not a surface",
                id.0
            )));
        } else {
            warn!("CloseSurface for unknown region {:?}", id);
            self.queue_protocol_error(NativeProtocolError::region_not_found(id));
        }
        Ok(())
    }

    /// Drop the per-region state of deleted regions, so an id created again
    /// later starts with none of it.
    fn forget_regions(&mut self, removed: &[RegionId]) {
        for id in removed {
            self.subscriptions.remove(id);
            self.scroll_offsets.remove(id);
            self.scroll_targets.remove(id);
            self.style_animations.remove(id);
            self.active_streams.remove(id);
        }
        self.forget_live_appends(removed);
        if self
            .focused_region
            .is_some_and(|focused| removed.contains(&focused))
        {
            self.focused_region = None;
        }
    }

    pub(super) fn send_surface_event(
        &mut self,
        id: RegionId,
        kind: SurfaceEventKind,
    ) -> io::Result<()> {
        self.send_message(&TerminalMessage::SurfaceEvent { id, kind })
    }

    /// Handle ReparentRegion.
    fn handle_reparent_region(&mut self, id: RegionId, new_parent: RegionId, insert_index: i32) {
        let index = usize::try_from(insert_index).unwrap_or(usize::MAX);
        match self.scene.reparent(id, new_parent, index) {
            Ok(()) => {
                self.mark_full_scene_rebuild();
                debug!("Reparented region {:?} to {:?}", id, new_parent);
            }
            Err(error) => {
                warn!(
                    "ReparentRegion {:?} to parent {:?} rejected: {}",
                    id, new_parent, error
                );
                self.queue_protocol_error(NativeProtocolError::scene_edit_rejected(
                    "reparent_region",
                    error,
                ));
            }
        }
    }
}
