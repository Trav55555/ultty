//! Native protocol command application.

use std::io;

use tracing::{debug, info, warn};
use ultty_protocol::{
    negotiate_capabilities, negotiate_protocol_version, AppMessage, Constraints, Content, RegionId,
    Style, TerminalMessage, CAP_APPEND_CONTENT, CAP_REGION_METADATA, CAP_STREAM_LIFECYCLE,
    MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};

use super::{NativeSession, StyleAnimation, TERMINAL_CAPABILITIES};
use crate::scene::region::Region;

impl NativeSession {
    /// Apply one decoded app message outside frame batching.
    pub(super) fn apply_message(&mut self, msg: AppMessage) -> io::Result<bool> {
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
                    self.queue_error_message(400, "set_region_metadata not negotiated".to_string());
                }
            }

            AppMessage::AppendContent { id, text } => {
                if self.has_capability(CAP_APPEND_CONTENT) {
                    self.handle_append_content(id, text);
                } else {
                    self.queue_error_message(400, "append_content not negotiated".to_string());
                }
            }

            AppMessage::StreamStart { id, stream_id } => {
                if self.has_capability(CAP_STREAM_LIFECYCLE) {
                    debug!("Stream {} started on region {:?}", stream_id, id);
                    self.active_streams.insert(id, stream_id);
                } else {
                    self.queue_error_message(400, "stream_start not negotiated".to_string());
                }
            }

            AppMessage::StreamEnd { id, stream_id } => {
                if self.has_capability(CAP_STREAM_LIFECYCLE) {
                    debug!("Stream {} ended on region {:?}", stream_id, id);
                    if let Some(active_id) = self.active_streams.get(&id) {
                        if *active_id == stream_id {
                            self.active_streams.remove(&id);
                        } else {
                            warn!(
                                "StreamEnd mismatch for {:?}: active={}, got={}",
                                id, active_id, stream_id
                            );
                        }
                    }
                } else {
                    self.queue_error_message(400, "stream_end not negotiated".to_string());
                }
            }

            AppMessage::DeleteRegion { id } => {
                self.handle_delete_region(id);
            }

            AppMessage::ReparentRegion {
                id,
                new_parent,
                insert_index,
            } => {
                self.handle_reparent_region(id, new_parent, insert_index);
            }

            AppMessage::BeginFrame { .. } | AppMessage::EndFrame { .. } => {
                self.queue_error_message(
                    400,
                    "frame control message reached command application".to_string(),
                );
            }

            AppMessage::Subscribe { id, events } => {
                self.subscriptions.insert(id, events);
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
                self.scene_dirty = true;
                self.scroll_dirty = true;
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

            AppMessage::Disconnect => {
                info!("Session {} disconnected (explicit)", self.session_id);
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Handle ClientHello - send ServerHello and SessionReady.
    fn handle_client_hello(&mut self, version: u16, requested: u32) -> io::Result<()> {
        info!(
            "Session {} client hello: version={}, caps=0x{:04x}",
            self.session_id, version, requested
        );

        let Some(negotiated_version) = negotiate_protocol_version(version, PROTOCOL_VERSION) else {
            self.queue_error_message(
                426,
                format!(
                    "unsupported protocol version {} (min supported {})",
                    version, MIN_PROTOCOL_VERSION
                ),
            );
            return Ok(());
        };

        self.protocol_version = negotiated_version;
        self.capabilities = negotiate_capabilities(requested, TERMINAL_CAPABILITIES);

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

    fn has_capability(&self, capability: u32) -> bool {
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

        if insert_index >= 0 {
            self.scene.insert_at(region, insert_index as usize);
        } else {
            self.scene.insert(region);
        }

        self.scene_dirty = true;
        self.constraints_dirty = true;
        self.render_dirty = true;
        debug!("Created region {:?} under {:?}", id, parent);
    }

    /// Handle SetConstraints.
    fn handle_set_constraints(&mut self, id: RegionId, constraints: Constraints) {
        if let Some(region) = self.scene.get_mut(id) {
            region.constraints = constraints;
            region.mark_dirty();
            self.scene_dirty = true;
            self.constraints_dirty = true;
            self.render_dirty = true;
            self.scroll_dirty = true;
            self.dirty_regions.insert(id);
        } else {
            warn!("SetConstraints for unknown region {:?}", id);
            self.queue_error_message(404, format!("region {} not found", id.0));
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
            self.scene_dirty = true;
            self.render_dirty = true;
            self.dirty_regions.insert(id);
        } else {
            warn!("SetStyle for unknown region {:?}", id);
            self.queue_error_message(404, format!("region {} not found", id.0));
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
        if let Some(region) = self.scene.get_mut(id) {
            region.content = content;
            region.mark_dirty();
            self.scene_dirty = true;
            self.render_dirty = true;
            self.dirty_regions.insert(id);
        } else {
            warn!("SetContent for unknown region {:?}", id);
            self.queue_error_message(404, format!("region {} not found", id.0));
        }
    }

    /// Handle AppendContent - append text to existing region content.
    fn handle_append_content(&mut self, id: RegionId, text: String) {
        if let Some(region) = self.scene.get_mut(id) {
            match &mut region.content {
                Content::Text(tc) => {
                    tc.text.push_str(&text);
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
                    self.queue_error_message(
                        400,
                        format!("cannot append to content type of region {}", id.0),
                    );
                    return;
                }
            }
            region.mark_dirty();
            self.scene_dirty = true;
            self.render_dirty = true;
            self.dirty_regions.insert(id);
        } else {
            warn!("AppendContent for unknown region {:?}", id);
            self.queue_error_message(404, format!("region {} not found", id.0));
        }
    }

    /// Handle SetRegionMetadata.
    fn handle_set_region_metadata(&mut self, id: RegionId, metadata: Vec<(String, String)>) {
        if let Some(region) = self.scene.get_mut(id) {
            region.metadata = metadata;
        } else {
            warn!("SetRegionMetadata for unknown region {:?}", id);
            self.queue_error_message(404, format!("region {} not found", id.0));
        }
    }

    /// Handle DeleteRegion.
    fn handle_delete_region(&mut self, id: RegionId) {
        if self.scene.remove(id).is_some() {
            self.scene_dirty = true;
            self.constraints_dirty = true;
            self.render_dirty = true;
            self.subscriptions.remove(&id);
            debug!("Deleted region {:?}", id);
        } else {
            warn!("DeleteRegion for unknown region {:?}", id);
            self.queue_error_message(404, format!("region {} not found", id.0));
        }
    }

    /// Handle ReparentRegion.
    fn handle_reparent_region(&mut self, id: RegionId, new_parent: RegionId, insert_index: i32) {
        let index = usize::try_from(insert_index).unwrap_or(usize::MAX);
        if self.scene.reparent(id, new_parent, index) {
            self.scene_dirty = true;
            self.constraints_dirty = true;
            self.render_dirty = true;
            debug!("Reparented region {:?} to {:?}", id, new_parent);
        } else {
            warn!(
                "ReparentRegion failed for region {:?} to parent {:?}",
                id, new_parent
            );
            self.queue_error_message(
                404,
                format!("region {} or parent {} not found", id.0, new_parent.0),
            );
        }
    }
}
