//! Control server request handling.
//!
//! Processes CLI automation requests from ulttyctl and other tools.

use super::control_resize::{PendingResize, RESIZE_NOT_APPLIED};
#[cfg(test)]
use super::control_wait::find_ignore_case;
use super::control_wait::{Condition, PendingControlWait, WaitMatcher};
use super::App;
use std::io;
use std::time::{Duration, Instant};
pub(super) use ultty::control::auth::write_auth_token;
#[cfg(test)]
use ultty::control::auth::AUTH_TOKEN_BYTES;
use ultty::control::auth::{control_request_requires_auth, read_auth_token, token_matches};
#[cfg(test)]
use ultty::control::common::plain_search_match;
pub(super) use ultty::control::common::terminal_working_directory;
use ultty::control::common::{
    buffer_text, build_command_line, control_pane_snapshot, input_acceptance, job_id,
    job_is_running, key_to_bytes, pane_infos, process_job_response, program_status_response,
    resolve_control_target, search_response, send_pty_input, shell_command_response,
    submit_control_shell_command, terminal_info,
};
use ultty::terminal_manager::{SplitDirection, TerminalId};
use winit::dpi::PhysicalSize;

impl App {
    pub(super) fn handle_control_requests(&mut self) {
        use ultty_protocol::control::{ControlRequest, ControlResponse};

        let requests = {
            let Some(ref mut server) = self.control_server else {
                return;
            };
            server.take_pending_requests()
        };

        let mut responses: Vec<(usize, ControlResponse)> = Vec::new();
        let mut subscriptions: Vec<(usize, Option<u64>)> = Vec::new();

        'requests: for (client_key, request) in requests {
            // Unwrap identity/authentication before any deferred or mutating path.
            let mut request = request;
            loop {
                match request {
                    ControlRequest::WithToken {
                        token,
                        request: inner,
                    } => {
                        if let Err(response) = self.authenticate_client(client_key, &token) {
                            responses.push((client_key, response));
                            continue 'requests;
                        }
                        request = *inner;
                    }
                    ControlRequest::WithOwner {
                        owner,
                        request: inner,
                    } => {
                        if let Some(response) = self.owner_error(&owner) {
                            responses.push((client_key, response));
                            continue 'requests;
                        }
                        request = *inner;
                    }
                    _ => break,
                }
            }

            if let ControlRequest::SubscribeV2 { ref cursor, target } = request {
                if let Some(response) = self.auth_error_for_request(client_key, &request) {
                    responses.push((client_key, response));
                    continue;
                }
                if cursor.is_none() && target.is_some() {
                    if let Err(response) = resolve_control_target(&self.runtime.manager, target) {
                        responses.push((client_key, response));
                        continue;
                    }
                }
                let snapshot = if cursor.is_none() {
                    control_pane_snapshot(&self.runtime.manager, target)
                } else {
                    Vec::new()
                };
                if let Some(server) = self.control_server.as_mut() {
                    server.start_subscription_v2(client_key, target, cursor.clone(), snapshot);
                }
                continue;
            }

            if matches!(request, ControlRequest::InvokeAction { .. }) {
                if let Some(response) = self.relay_semantic_action(client_key, &request) {
                    responses.push((client_key, response));
                }
                continue;
            }

            if let ControlRequest::WaitJob { job, timeout_ms } = request {
                if let Some(response) = self.auth_error_for_request(client_key, &request) {
                    responses.push((client_key, response));
                    continue;
                }
                let response = job_id(job).and_then(|id| {
                    if timeout_ms == 0 || timeout_ms > 86_400_000 {
                        return Err(ControlResponse::Error {
                            code: 4,
                            message: "Job timeout must be between 1 and 86400000 ms".into(),
                        });
                    }
                    let response = process_job_response(&self.runtime.manager, id);
                    if job_is_running(&response) {
                        self.pending_control_waits.push(PendingControlWait::new(
                            client_key,
                            id,
                            Condition::ProcessJob,
                            Duration::from_millis(timeout_ms),
                            Instant::now(),
                        ));
                        Ok(None)
                    } else {
                        Ok(Some(response))
                    }
                });
                match response {
                    Ok(Some(response)) | Err(response) => responses.push((client_key, response)),
                    Ok(None) => {}
                }
                continue;
            }

            if matches!(request, ControlRequest::RunCommandAndWait { .. }) {
                if let Some(response) = self.defer_shell_command(client_key, &request) {
                    responses.push((client_key, response));
                }
                continue;
            }

            if let ControlRequest::Subscribe { target } = request {
                match self.subscription_target(client_key, &request, target) {
                    Ok(target) => subscriptions.push((client_key, target)),
                    Err(response) => responses.push((client_key, response)),
                }
                continue;
            }

            if let Some(response) = self.defer_wait_for_request(client_key, &request) {
                responses.push((client_key, response));
                continue;
            }

            if self.is_wait_for_request(&request) {
                continue;
            }

            if is_override_request(&request) {
                // Answered when the app does, or at once with why it cannot.
                if let Some(response) = self.relay_override_request(client_key, request) {
                    responses.push((client_key, response));
                }
                continue;
            }

            if let ControlRequest::Resize { width, height } = request {
                // Answered once the window has the size, or at once when
                // the platform resizes synchronously or refuses.
                let requested = PhysicalSize::new(width, height);
                if let Some(response) = self.request_window_resize(client_key, &request, requested)
                {
                    responses.push((client_key, response));
                }
                continue;
            }

            let response = self.process_control_request(client_key, request);
            responses.push((client_key, response));
        }

        if let Some(ref mut server) = self.control_server {
            for (client_key, response) in responses {
                server.send_response(client_key, &response);
            }
            for (client_key, target) in subscriptions {
                server.start_subscription(client_key, target);
            }
        }
    }

    fn owner_error(
        &self,
        owner: &ultty_protocol::control::OwnerRef,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        let current = self.control_server.as_ref().map(|server| server.owner());
        (current != Some(owner)).then(|| ultty_protocol::control::ControlResponse::Error {
            code: 409,
            message: "Control owner changed; rediscover explicitly before retrying".into(),
        })
    }

    /// Check a `subscribe` request: it needs the token, and a `target` must
    /// name an open pane. Returns the pane filter to subscribe with.
    fn subscription_target(
        &self,
        client_key: usize,
        request: &ultty_protocol::control::ControlRequest,
        target: Option<u64>,
    ) -> Result<Option<u64>, ultty_protocol::control::ControlResponse> {
        if let Some(response) = self.auth_error_for_request(client_key, request) {
            return Err(response);
        }
        if target.is_some() {
            resolve_control_target(&self.runtime.manager, target)?;
        }
        Ok(target)
    }

    /// The earliest instant a pending wait, override push or resize needs
    /// the UI awake for.
    pub(super) fn next_control_wait_deadline(&self) -> Option<Instant> {
        self.pending_control_waits
            .iter()
            .map(PendingControlWait::deadline)
            .chain(self.pending_override_pushes.iter().map(|push| push.expires))
            .chain(
                self.pending_semantic_actions
                    .iter()
                    .map(|pending| pending.expires),
            )
            .chain(self.pending_resizes.iter().map(PendingResize::deadline))
            .min()
    }

    /// Ask the platform for a window of `requested` size. `None` means the
    /// platform resizes later and the reply waits for it (see
    /// `control_resize`); `Some` is the answer now.
    fn request_window_resize(
        &mut self,
        client_key: usize,
        request: &ultty_protocol::control::ControlRequest,
        requested: PhysicalSize<u32>,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::ControlResponse;

        if let Some(response) = self.auth_error_for_request(client_key, request) {
            return Some(response);
        }
        let result = self.window.request_inner_size(requested);
        if result.is_some() {
            let response = resize_control_response(result, requested, self.window.inner_size());
            if matches!(response, ControlResponse::Ok) {
                self.handle_resize(requested);
            }
            return Some(response);
        }
        self.pending_resizes
            .push(PendingResize::new(client_key, requested, Instant::now()));
        // The window may already have the size: a no-op request gets no
        // `Resized`, and without a window manager X11 applies it at once.
        self.answer_pending_resizes(self.window.inner_size());
        None
    }

    /// Answer the pending resizes that a window of `current` size settles:
    /// those it matches, and those past their deadline. A window that has
    /// the size before its `Resized` arrives is laid out first, so the
    /// client sees the new pane sizes once answered.
    pub(super) fn answer_pending_resizes(&mut self, current: PhysicalSize<u32>) {
        use ultty_protocol::control::ControlResponse;

        if self.pending_resizes.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut responses = Vec::new();
        self.pending_resizes
            .retain(|resize| match resize.reply(current, now) {
                Some(response) => {
                    responses.push((resize.client_key(), response));
                    false
                }
                None => true,
            });
        let matched = responses
            .iter()
            .any(|(_, response)| matches!(response, ControlResponse::Ok));
        if matched && self.renderer.surface_size() != (current.width, current.height) {
            self.handle_resize(current);
        }
        if let Some(ref mut server) = self.control_server {
            for (client_key, response) in responses {
                server.send_response(client_key, &response);
            }
        }
    }

    /// Answer the pending resizes whose deadline has passed, with the size
    /// the window has.
    pub(super) fn expire_pending_resizes(&mut self) {
        let now = Instant::now();
        if self
            .pending_resizes
            .iter()
            .any(|resize| now >= resize.deadline())
        {
            self.answer_pending_resizes(self.window.inner_size());
        }
    }

    /// Send a `PushOverride` or `ClearOverride` on to the native app in the
    /// target pane's tab (ADR 0023 §9). `None` means the request now waits
    /// for the app's `OverrideResult`; `Some` is an immediate answer, which
    /// is always an error: no app to send it to, or a push too large.
    fn relay_override_request(
        &mut self,
        client_key: usize,
        request: ultty_protocol::control::ControlRequest,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::{ControlRequest, ControlResponse};
        use ultty_protocol::{PushedOverride, MAX_OVERRIDE_SOURCE_LEN};

        if let Some(response) = self.auth_error_for_request(client_key, &request) {
            return Some(response);
        }
        let (pushed, target) = match request {
            ControlRequest::PushOverride {
                format,
                source,
                target,
            } => (Some(PushedOverride { format, source }), target),
            ControlRequest::ClearOverride { target } => (None, target),
            _ => return None,
        };
        if let Some(pushed) = &pushed {
            if pushed.source.len() > MAX_OVERRIDE_SOURCE_LEN {
                return Some(ControlResponse::Error {
                    code: OVERRIDE_REJECTED,
                    message: format!(
                        "The override is {} bytes; at most {MAX_OVERRIDE_SOURCE_LEN} can be pushed",
                        pushed.source.len()
                    ),
                });
            }
        }
        let pane = match resolve_control_target(&self.runtime.manager, target) {
            Ok(pane) => pane,
            Err(response) => return Some(response),
        };
        let session_key = match self.override_target_session(pane) {
            Ok(key) => key,
            Err(response) => return Some(response),
        };
        let request_id = self.next_override_request;
        self.next_override_request = self.next_override_request.wrapping_add(1);
        let sent = self
            .runtime
            .protocol
            .as_mut()
            .map(|server| server.send_set_override(session_key, request_id, pushed))
            .unwrap_or_else(|| Err(io::Error::other("no protocol server")));
        if let Err(error) = sent {
            return Some(ControlResponse::Error {
                code: NO_NATIVE_APP,
                message: format!(
                    "Cannot reach the native app in pane {}: {error}",
                    pane.raw()
                ),
            });
        }
        self.pending_override_pushes.push(PendingOverridePush {
            client_key,
            session_key,
            request_id,
            expires: Instant::now() + OVERRIDE_REPLY_TIMEOUT,
        });
        None
    }

    fn relay_semantic_action(
        &mut self,
        client_key: usize,
        request: &ultty_protocol::control::ControlRequest,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        if let Some(response) = self.auth_error_for_request(client_key, request) {
            return Some(response);
        }
        let ultty_protocol::control::ControlRequest::InvokeAction {
            target,
            session,
            expected_generation,
            region,
            widget_id,
            action,
        } = request
        else {
            return None;
        };
        self.submit_semantic_action(
            Some(client_key),
            SemanticActionTarget {
                pane: *target,
                session: *session,
                generation: *expected_generation,
                region: *region,
                widget_id: widget_id.clone(),
                action: *action,
            },
        )
        .err()
    }

    /// Route an action pinned to the accessibility tree that produced it.
    /// Application replies follow the same correlation path as control calls.
    pub(super) fn dispatch_accessibility_action(
        &mut self,
        pane: u64,
        session: u64,
        generation: u64,
        region: u32,
        widget_id: String,
        action: ultty_protocol::SemanticActionKind,
    ) {
        if let Err(response) = self.submit_semantic_action(
            None,
            SemanticActionTarget {
                pane,
                session,
                generation,
                region,
                widget_id,
                action,
            },
        ) {
            tracing::warn!(?response, "Accessibility action dispatch failed");
        }
    }

    fn submit_semantic_action(
        &mut self,
        client_key: Option<usize>,
        target: SemanticActionTarget,
    ) -> Result<(), ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::ControlResponse;
        if !ultty_protocol::valid_action_widget_id(&target.widget_id) {
            return Err(ControlResponse::Error {
                code: 4,
                message: "Invalid semantic widget identity".into(),
            });
        }
        if self.pending_semantic_actions.len() >= MAX_PENDING_CONTROL_ACTIONS {
            return Err(ControlResponse::Error {
                code: INPUT_QUEUE_FULL,
                message: "Semantic action acknowledgement capacity is full".into(),
            });
        }
        let pane = resolve_control_target(&self.runtime.manager, Some(target.pane))?;
        let Some(server) = self.runtime.protocol.as_mut() else {
            return Err(ControlResponse::Error {
                code: NO_NATIVE_APP,
                message: "No native protocol server".into(),
            });
        };
        let request_id = server
            .request_action(
                target.session,
                pane,
                target.generation,
                ultty_protocol::RegionId(target.region),
                &target.widget_id,
                target.action,
            )
            .map_err(|error| ControlResponse::Error {
                code: if error.kind() == io::ErrorKind::WouldBlock {
                    INPUT_QUEUE_FULL
                } else {
                    ACTION_REJECTED
                },
                message: format!(
                    "Semantic action dispatch failed: {error}; reinspect before retrying"
                ),
            })?;
        self.pending_semantic_actions.push(PendingSemanticAction {
            client_key,
            session_id: target.session,
            request_id,
            expires: Instant::now() + ACTION_REPLY_TIMEOUT,
        });
        Ok(())
    }

    /// Finish actions only after their exact application acknowledgement,
    /// disconnect, or deadline. Cancelling bookkeeping cannot undo app work.
    pub(super) fn complete_pending_semantic_actions(&mut self) {
        if self.pending_semantic_actions.is_empty() {
            return;
        }
        let results = self
            .runtime
            .protocol
            .as_mut()
            .map(|server| server.take_action_results())
            .unwrap_or_default();
        let now = Instant::now();
        let pending = std::mem::take(&mut self.pending_semantic_actions);
        let mut responses = Vec::new();
        for action in pending {
            let connected = self
                .runtime
                .protocol
                .as_ref()
                .is_some_and(|server| server.has_session_id(action.session_id));
            let Some(response) = action.response(&results, connected, now) else {
                self.pending_semantic_actions.push(action);
                continue;
            };
            if let Some(server) = self.runtime.protocol.as_mut() {
                server.cancel_action(action.session_id, action.request_id);
            }
            if let Some(client_key) = action.client_key {
                responses.push((client_key, response));
            } else {
                tracing::debug!(
                    session = action.session_id,
                    request_id = action.request_id,
                    ?response,
                    "Accessibility action completed"
                );
            }
        }
        if let Some(server) = self.control_server.as_mut() {
            for (key, response) in responses {
                server.send_response(key, &response);
            }
        }
    }

    /// The session an override for `pane` goes to: the native session in
    /// the pane's tab whose process is in the pane's foreground process
    /// group, as key input is routed. Until sessions are bound to panes
    /// (ADR 0025) this is the best the terminal knows.
    fn override_target_session(
        &self,
        pane: TerminalId,
    ) -> Result<usize, ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::ControlResponse;

        let no_app = |message: String| ControlResponse::Error {
            code: NO_NATIVE_APP,
            message,
        };
        let tab = self.runtime.manager.terminal(pane).map(|t| t.tab_id);
        let server = self.runtime.protocol.as_ref().ok_or_else(|| {
            no_app("This Ultty is running without its native protocol server".to_string())
        })?;
        let in_tab: Vec<(usize, &ultty::protocol::NativeSession)> = server
            .sessions_with_keys()
            .filter(|(_, session)| Some(session.tab_id()) == tab)
            .collect();
        let foreground = self.pane_foreground_pgrp(pane);
        pick_override_session(&in_tab, pane, foreground).map_err(|reason| {
            no_app(match reason {
                NoOverrideTarget::NoneForPane { unbound: 0 } => format!(
                    "No native app in pane {}; an app drawing with ANSI reads its override files instead",
                    pane.raw()
                ),
                NoOverrideTarget::NoneForPane { unbound } => format!(
                    "No native app in pane {} ({unbound} in its tab not bound to a pane and not in its foreground); an app drawing with ANSI reads its override files instead",
                    pane.raw()
                ),
                NoOverrideTarget::CannotTakePushes => format!(
                    "The native app in pane {} does not take pushed overrides (it was built before CAP_OVERRIDE_PUSH)",
                    pane.raw()
                ),
                NoOverrideTarget::TooManyOutstanding => format!(
                    "The native app in pane {} has {} override pushes unanswered; try again when it answers",
                    pane.raw(),
                    ultty::protocol::MAX_AWAITED_OVERRIDE_RESULTS
                ),
            })
        })
    }

    /// Answer the override pushes whose app has replied, whose app is gone,
    /// or whose reply is overdue.
    pub(super) fn complete_pending_override_pushes(&mut self) {
        use ultty_protocol::control::ControlResponse;

        if self.pending_override_pushes.is_empty() {
            return;
        }
        let Some(server) = self.runtime.protocol.as_mut() else {
            return;
        };
        let results = server.take_override_results();
        let now = Instant::now();
        let mut responses = Vec::new();
        let pending = std::mem::take(&mut self.pending_override_pushes);
        for push in pending {
            let answered = results
                .iter()
                .find(|(key, id, _)| *key == push.session_key && *id == push.request_id);
            let response = if let Some((_, _, error)) = answered {
                match error {
                    None => ControlResponse::Ok,
                    Some(error) => ControlResponse::Error {
                        code: OVERRIDE_REJECTED,
                        message: error.clone(),
                    },
                }
            } else if !server.has_session(push.session_key) {
                ControlResponse::Error {
                    code: NO_NATIVE_APP,
                    message: "The native app disconnected before answering the override"
                        .to_string(),
                }
            } else if now >= push.expires {
                // A late answer is dropped rather than queued for nobody.
                server.forget_override_result(push.session_key, push.request_id);
                ControlResponse::Error {
                    code: OVERRIDE_REJECTED,
                    message: format!(
                        "The native app did not answer the override within {} s",
                        OVERRIDE_REPLY_TIMEOUT.as_secs()
                    ),
                }
            } else {
                self.pending_override_pushes.push(push);
                continue;
            };
            responses.push((push.client_key, response));
        }
        if let Some(ref mut control) = self.control_server {
            for (client_key, response) in responses {
                control.send_response(client_key, &response);
            }
        }
    }

    /// Answer the waits on `pane` that were waiting for `event`, and restart
    /// quiet periods on output.
    pub(super) fn observe_control_wait_event(
        &mut self,
        pane: TerminalId,
        event: &ultty::event_loop::TerminalEvent,
    ) {
        use ultty_protocol::control::{ControlData, ControlResponse};

        if self.pending_control_waits.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut answered = Vec::new();
        self.pending_control_waits.retain_mut(|wait| {
            if wait.pane() != pane || !wait.observe(event, now) {
                return true;
            }
            let response = match event {
                ultty::event_loop::TerminalEvent::ShellCommandFinished {
                    sequence,
                    exit_code,
                } => Some(ControlResponse::Data(ControlData::ShellCommandFinished {
                    pane: pane.raw() as u64,
                    sequence: *sequence,
                    exit_code: *exit_code,
                })),
                event => super::control_events::control_event(pane, event)
                    .map(|event| ControlResponse::Data(ControlData::WaitEvent { event })),
            };
            if let Some(response) = response {
                answered.push((wait.client_key(), response));
            }
            false
        });
        if let Some(ref mut server) = self.control_server {
            for (client_key, response) in answered {
                server.send_response(client_key, &response);
            }
        }
    }

    pub(super) fn complete_pending_control_waits(&mut self) {
        let mut still_pending = Vec::new();
        let mut responses = Vec::new();
        let pending = std::mem::take(&mut self.pending_control_waits);

        for wait in pending {
            if let Some(response) = self.pending_wait_response(&wait) {
                responses.push((wait.client_key(), response));
            } else {
                still_pending.push(wait);
            }
        }

        self.pending_control_waits = still_pending;

        if let Some(ref mut server) = self.control_server {
            for (client_key, response) in responses {
                server.send_response(client_key, &response);
            }
        }
    }

    fn is_wait_for_request(&self, request: &ultty_protocol::control::ControlRequest) -> bool {
        matches!(
            request,
            ultty_protocol::control::ControlRequest::WaitFor { .. }
        )
    }

    fn defer_wait_for_request(
        &mut self,
        client_key: usize,
        request: &ultty_protocol::control::ControlRequest,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::{
            ControlData, ControlRequest, ControlResponse, WaitCondition,
        };

        let ControlRequest::WaitFor {
            pattern,
            regex: use_regex,
            timeout_ms,
            until,
            target,
        } = request
        else {
            return None;
        };

        if let Some(response) = self.auth_error_for_request(client_key, request) {
            return Some(response);
        }

        let pane = match resolve_control_target(&self.runtime.manager, *target) {
            Ok(pane) => pane,
            Err(response) => return Some(response),
        };

        let now = Instant::now();
        let condition = match until {
            WaitCondition::Text => {
                if pattern.is_empty() {
                    return Some(ControlResponse::Error {
                        code: 4,
                        message: "wait-for needs a pattern or an event to wait for".to_string(),
                    });
                }
                let matcher = match WaitMatcher::new(pattern, *use_regex) {
                    Ok(matcher) => matcher,
                    Err(message) => return Some(ControlResponse::Error { code: 4, message }),
                };
                if let Some((line, col)) = self.find_wait_match(pane, &matcher) {
                    return Some(ControlResponse::Data(ControlData::WaitMatched {
                        line,
                        col,
                    }));
                }
                Condition::Text(matcher)
            }
            WaitCondition::CommandFinished => Condition::CommandFinished,
            WaitCondition::Bell => Condition::Bell,
            WaitCondition::Quiet { ms } => Condition::quiet(Duration::from_millis(*ms), now),
        };

        self.pending_control_waits.push(PendingControlWait::new(
            client_key,
            pane,
            condition,
            Duration::from_millis(*timeout_ms),
            now,
        ));
        None
    }

    fn defer_shell_command(
        &mut self,
        client_key: usize,
        request: &ultty_protocol::control::ControlRequest,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::{ControlRequest, ControlResponse};

        let ControlRequest::RunCommandAndWait {
            command,
            timeout_ms,
            target,
        } = request
        else {
            return Some(ControlResponse::Error {
                code: 4,
                message: "Expected a shell submission".to_string(),
            });
        };
        if let Some(response) = self.auth_error_for_request(client_key, request) {
            return Some(response);
        }
        if *timeout_ms == 0 || *timeout_ms > 86_400_000 {
            return Some(ControlResponse::Error {
                code: 4,
                message: "Shell completion timeout must be between 1 and 86400000 ms".to_string(),
            });
        }
        let pane = match resolve_control_target(&self.runtime.manager, *target) {
            Ok(pane) => pane,
            Err(response) => return Some(response),
        };
        let sequence = match submit_control_shell_command(&mut self.runtime.manager, pane, command)
        {
            Ok(sequence) => sequence,
            Err(response) => return Some(response),
        };
        self.pending_control_waits.push(PendingControlWait::new(
            client_key,
            pane,
            Condition::ShellCommand { sequence },
            Duration::from_millis(*timeout_ms),
            Instant::now(),
        ));
        None
    }

    fn pending_wait_response(
        &self,
        wait: &PendingControlWait,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::{ControlData, ControlResponse};

        if wait.is_process_job() {
            let response = process_job_response(&self.runtime.manager, wait.pane());
            return if !job_is_running(&response) {
                Some(response)
            } else if wait.is_expired(Instant::now()) {
                Some(ControlResponse::Data(ControlData::WaitTimeout))
            } else {
                None
            };
        }

        if let Some(sequence) = wait.command_sequence() {
            if let Some(terminal) = self.runtime.manager.terminal(wait.pane()) {
                if let Some(outcome) = terminal.event_loop.shell_command_result(sequence) {
                    return Some(shell_command_response(wait.pane(), sequence, outcome));
                }
            }
        }

        if self.runtime.manager.terminal(wait.pane()).is_none() {
            return Some(ControlResponse::Error {
                code: PANE_NOT_FOUND,
                message: format!("Pane {} closed while waiting", wait.pane().raw()),
            });
        }

        if let Some((line, col)) = wait
            .matcher()
            .and_then(|matcher| self.find_wait_match(wait.pane(), matcher))
        {
            return Some(ControlResponse::Data(ControlData::WaitMatched {
                line,
                col,
            }));
        }

        let now = Instant::now();
        if wait.is_quiet(now) {
            return Some(ControlResponse::Data(ControlData::WaitQuiet));
        }

        if wait.is_expired(now) {
            return Some(ControlResponse::Data(ControlData::WaitTimeout));
        }

        None
    }

    /// Process a single control request and return the response.
    /// This is extracted to allow recursive calls for batch commands.
    fn process_control_request(
        &mut self,
        client_key: usize,
        request: ultty_protocol::control::ControlRequest,
    ) -> ultty_protocol::control::ControlResponse {
        use ultty_protocol::control::{
            ControlData, ControlRequest, ControlResponse, TabInfo, VersionInfo,
        };

        if let Some(response) = self.auth_error_for_request(client_key, &request) {
            return response;
        }

        match request {
            ControlRequest::Ping => ControlResponse::Data(ControlData::Pong),
            ControlRequest::JobStatus { job } => match job_id(job) {
                Ok(id) => process_job_response(&self.runtime.manager, id),
                Err(response) => response,
            },
            ControlRequest::WaitJob { .. } | ControlRequest::InvokeAction { .. } => {
                ControlResponse::Error {
                    code: 5,
                    message:
                        "Deferred jobs and semantic actions cannot be used inside batch requests"
                            .into(),
                }
            }
            ControlRequest::Describe => self
                .control_server
                .as_ref()
                .map(|server| server.description())
                .unwrap_or_else(|| ControlResponse::Error {
                    code: 1,
                    message: "Control owner unavailable".into(),
                }),
            ControlRequest::WithOwner { owner, request } => match self.owner_error(&owner) {
                Some(response) => response,
                None => self.process_control_request(client_key, *request),
            },

            ControlRequest::Version => ControlResponse::Data(ControlData::Version(VersionInfo {
                version: env!("CARGO_PKG_VERSION").to_string(),
                protocol_version: ultty_protocol::PROTOCOL_VERSION,
                git_hash: option_env!("GIT_HASH").map(String::from),
            })),

            ControlRequest::GetInfo => {
                if let Some(terminal) = self.runtime.manager.focused_terminal() {
                    let active_idx = self
                        .runtime
                        .manager
                        .tabs()
                        .iter()
                        .position(|t| {
                            self.runtime
                                .manager
                                .active_tab()
                                .is_some_and(|a| a.id == t.id)
                        })
                        .unwrap_or(0);
                    ControlResponse::Data(ControlData::Info(terminal_info(
                        terminal,
                        active_idx,
                        self.runtime.manager.tab_count(),
                    )))
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::ListTabs => {
                let active_tab_id = self.runtime.manager.active_tab().map(|t| t.id);
                let tabs: Vec<TabInfo> = self
                    .runtime
                    .manager
                    .tabs()
                    .iter()
                    .enumerate()
                    .map(|(i, tab)| TabInfo {
                        index: i,
                        title: self.runtime.manager.tab_display_title(tab.id),
                        active: Some(tab.id) == active_tab_id,
                        pid: self
                            .runtime
                            .manager
                            .terminal(tab.focused_terminal)
                            .map(|terminal| terminal.event_loop.child_pid()),
                        cwd: self
                            .runtime
                            .manager
                            .terminal(tab.focused_terminal)
                            .and_then(terminal_working_directory),
                    })
                    .collect();
                ControlResponse::Data(ControlData::Tabs { tabs })
            }

            ControlRequest::ListPanes => ControlResponse::Data(ControlData::Panes {
                panes: pane_infos(&self.runtime.manager),
            }),

            ControlRequest::WorkspaceList => match ultty::session::SessionData::workspace_names() {
                Ok(names) => ControlResponse::Data(ControlData::Workspaces { names }),
                Err(error) => ControlResponse::Error {
                    code: 2,
                    message: format!("Cannot list workspaces: {error}"),
                },
            },
            ControlRequest::WorkspaceSave { name } => {
                match self.runtime.manager.to_session_data().save_named(&name) {
                    Ok(()) => ControlResponse::Ok,
                    Err(error) => ControlResponse::Error {
                        code: 2,
                        message: format!("Cannot save workspace: {error}"),
                    },
                }
            }

            ControlRequest::ProgramStatus { target } => {
                match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => program_status_response(&self.runtime.manager, pane),
                    Err(response) => response,
                }
            }

            ControlRequest::GetText {
                include_scrollback,
                scrollback_lines,
                target,
            } => {
                let pane = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                if let Some(terminal) = self.runtime.manager.terminal(pane) {
                    let term = terminal.event_loop.terminal();
                    let history = if include_scrollback {
                        scrollback_lines.unwrap_or(usize::MAX)
                    } else {
                        0
                    };
                    ControlResponse::Text {
                        text: buffer_text(term.buffer(), history),
                    }
                } else {
                    ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    }
                }
            }

            ControlRequest::LastCommand { tail_lines, target } => {
                let pane = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let last = self.runtime.manager.terminal(pane).and_then(|terminal| {
                    let tail = tail_lines.map(|lines| lines.min(usize::MAX as u64) as usize);
                    terminal.event_loop.terminal().last_command(tail)
                });
                match last {
                    Some(last) => ControlResponse::Data(ControlData::LastCommand {
                        command: last.command,
                        output: last.output,
                        exit_code: last.exit_code,
                        running: last.running,
                        output_available: last.output_available,
                    }),
                    None => ControlResponse::Error {
                        code: NO_COMMAND_RECORDED,
                        message: format!(
                            "No command recorded in pane {}; it needs a shell with OSC 133 \
                             integration",
                            pane.raw()
                        ),
                    },
                }
            }

            ControlRequest::Regions { target } => {
                let pane = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                // The sessions drawn in the pane, plus its tab's sessions
                // that have no pane yet (a background job in a split): still
                // worth inspecting, drawn nowhere (ADR 0025 §5).
                let tab = self.runtime.manager.terminal(pane).map(|t| t.tab_id);
                let scenes = self
                    .runtime
                    .protocol
                    .as_ref()
                    .map(|server| {
                        server
                            .sessions()
                            .filter(|session| {
                                Some(session.tab_id()) == tab
                                    && session.pane().is_none_or(|p| p == pane)
                            })
                            .map(|session| session.describe())
                            .collect()
                    })
                    .unwrap_or_default();
                ControlResponse::Data(ControlData::Regions { scenes })
            }

            ControlRequest::GetSelection { target } => {
                let pane = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let text = self
                    .runtime
                    .manager
                    .terminal(pane)
                    .and_then(|terminal| terminal.event_loop.terminal().selected_text());
                ControlResponse::Data(ControlData::Selection { text })
            }

            ControlRequest::SendText { text, target } => {
                let pane = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                // The pane's native app takes it whether or not its tab is
                // shown; else it is typed into the pane's PTY.
                let foreground = self.pane_foreground_pgrp(pane);
                let tab = self.runtime.manager.terminal(pane).map(|t| t.tab_id);
                if let (Some(server), Some(tab)) = (self.runtime.protocol.as_mut(), tab) {
                    if server.has_pane_key_subscribers(tab, pane, foreground) {
                        return input_acceptance(
                            server.send_text_input_to_pane(tab, pane, text, foreground),
                        );
                    }
                }

                send_pty_input(&mut self.runtime.manager, pane, text.as_bytes())
            }

            ControlRequest::SendKey {
                key,
                modifiers,
                target,
            } => {
                let pane = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let foreground = self.pane_foreground_pgrp(pane);
                let tab = self.runtime.manager.terminal(pane).map(|t| t.tab_id);
                if let (Some(server), Some(tab)) = (self.runtime.protocol.as_mut(), tab) {
                    if server.has_pane_key_subscribers(tab, pane, foreground) {
                        return input_acceptance(
                            server.send_key_to_pane(tab, pane, key, modifiers, foreground),
                        );
                    }
                }

                self.forward_key_to_pty(pane, &key, modifiers)
            }

            ControlRequest::SendRaw { data, target } => {
                let pane = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                send_pty_input(&mut self.runtime.manager, pane, &data)
            }

            ControlRequest::NewTab {
                title,
                command,
                args,
                working_dir,
            } => {
                if command.is_none() && (!args.is_empty() || working_dir.is_some()) {
                    return invalid_spawn_options();
                }
                let result = if let Some(cmd) = command {
                    self.create_tab_with_command(
                        &cmd,
                        &args,
                        working_dir.as_deref(),
                        title.as_deref(),
                    )
                } else {
                    let result = self.create_tab();
                    if let (Ok(tab_id), Some(title)) = (result.as_ref(), title.as_deref()) {
                        self.runtime.manager.set_tab_title(*tab_id, title);
                    }
                    result
                };

                match result {
                    Ok(tab_id) => {
                        self.clear_tab_width_freeze();
                        self.update_tab_bar();
                        self.needs_redraw = true;
                        let tabs = self.runtime.manager.tabs();
                        match tabs.iter().position(|tab| tab.id == tab_id) {
                            Some(tab_index) => ControlResponse::Data(ControlData::TabOpened {
                                tab_index,
                                pane: tabs[tab_index].focused_terminal.raw() as u64,
                                warning: None,
                            }),
                            // The command exited at once and the tab closed.
                            None => ControlResponse::Error {
                                code: 2,
                                message: "The tab closed as soon as it opened".to_string(),
                            },
                        }
                    }
                    Err(e) => ControlResponse::Error {
                        code: 2,
                        message: format!("Failed to create tab: {}", e),
                    },
                }
            }

            ControlRequest::SplitPane {
                direction,
                command,
                args,
                working_dir,
                target,
            } => {
                if command.is_none() && (!args.is_empty() || working_dir.is_some()) {
                    return invalid_spawn_options();
                }
                let target = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let direction = match direction {
                    ultty_protocol::control::SplitDirection::Right => SplitDirection::Horizontal,
                    ultty_protocol::control::SplitDirection::Down => SplitDirection::Vertical,
                };
                let result = match command {
                    Some(command) => self.runtime.manager.split_pane_with_command(
                        target,
                        direction,
                        &command,
                        &args,
                        working_dir.as_deref(),
                    ),
                    None => self.runtime.manager.split_pane(target, direction),
                };
                match result {
                    Ok(Some(pane)) => {
                        self.needs_redraw = true;
                        let tab_index = self
                            .runtime
                            .manager
                            .tabs()
                            .iter()
                            .position(|tab| tab.terminals.contains(&pane));
                        match tab_index {
                            Some(tab_index) => ControlResponse::Data(ControlData::PaneOpened {
                                tab_index,
                                pane: pane.raw() as u64,
                                warning: None,
                            }),
                            // The command exited at once and the pane closed.
                            None => ControlResponse::Error {
                                code: 2,
                                message: "The pane closed as soon as it opened".to_string(),
                            },
                        }
                    }
                    Ok(None) => ControlResponse::Error {
                        code: PANE_NOT_FOUND,
                        message: format!("No pane {}; `ulttyctl panes` lists them", target.raw()),
                    },
                    Err(e) => ControlResponse::Error {
                        code: 2,
                        message: format!("Failed to split: {}", e),
                    },
                }
            }

            ControlRequest::SwitchTab { index } => {
                if index < self.runtime.manager.tab_count() {
                    self.runtime.manager.switch_to_tab(index);
                    self.update_tab_bar();
                    self.needs_redraw = true;
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 3,
                        message: format!("Tab index {} out of range", index),
                    }
                }
            }

            ControlRequest::CloseTab { index } => {
                let tabs = self.runtime.manager.tabs();
                let active_idx = tabs
                    .iter()
                    .position(|t| {
                        self.runtime
                            .manager
                            .active_tab()
                            .is_some_and(|a| a.id == t.id)
                    })
                    .unwrap_or(0);
                let idx = index.unwrap_or(active_idx);
                if self.runtime.manager.tab_count() > 1 && idx < tabs.len() {
                    let tab_id = tabs[idx].id;
                    self.freeze_tab_width_after_close();
                    self.runtime.manager.close_tab(tab_id);
                    self.update_tab_bar();
                    self.needs_redraw = true;
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 4,
                        message: "Cannot close last tab or invalid index".to_string(),
                    }
                }
            }

            ControlRequest::RunCommand {
                command,
                args,
                target,
            } => {
                let pane = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let cmd_line = build_command_line(&command, &args);
                send_pty_input(&mut self.runtime.manager, pane, cmd_line.as_bytes())
            }

            ControlRequest::RunCommandAndWait { .. } => ControlResponse::Error {
                code: 4,
                message: "run-and-wait cannot be used inside a batch".to_string(),
            },

            ControlRequest::FocusPane { target } => {
                let pane = match resolve_control_target(&self.runtime.manager, Some(target)) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                if self.runtime.manager.focus_pane_by_id(pane) {
                    self.sync_protocol_tabs();
                    self.update_tab_bar();
                    self.needs_redraw = true;
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: PANE_NOT_FOUND,
                        message: "The target pane is gone".to_string(),
                    }
                }
            }
            ControlRequest::ZoomPane { target, zoomed } => {
                let pane = match resolve_control_target(&self.runtime.manager, Some(target)) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                if self.runtime.manager.set_pane_zoom(pane, zoomed) {
                    self.sync_protocol_tabs();
                    self.update_tab_bar();
                    self.needs_redraw = true;
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 4,
                        message: "Cannot zoom the target pane".to_string(),
                    }
                }
            }
            ControlRequest::SwapPanes { first, second } => {
                let first = match resolve_control_target(&self.runtime.manager, Some(first)) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let second = match resolve_control_target(&self.runtime.manager, Some(second)) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                if self.runtime.manager.swap_panes(first, second) {
                    self.update_tab_bar();
                    self.needs_redraw = true;
                    ControlResponse::Ok
                } else {
                    ControlResponse::Error {
                        code: 4,
                        message: "Panes must belong to the same tab to swap".to_string(),
                    }
                }
            }

            ControlRequest::FocusWindow => {
                self.window.focus_window();
                ControlResponse::Ok
            }

            ControlRequest::ToggleFullscreen => {
                use winit::window::Fullscreen;
                let current = self.window.fullscreen();
                if current.is_some() {
                    self.window.set_fullscreen(None);
                } else {
                    self.window
                        .set_fullscreen(Some(Fullscreen::Borderless(None)));
                }
                ControlResponse::Ok
            }

            // A top-level resize goes through `request_window_resize`,
            // which can wait for the window; a batch cannot.
            ControlRequest::Resize { width, height } => {
                let requested = PhysicalSize::new(width, height);
                let result = self.window.request_inner_size(requested);
                let response = resize_control_response(result, requested, self.window.inner_size());
                if matches!(response, ControlResponse::Ok) {
                    self.handle_resize(requested);
                }
                response
            }

            ControlRequest::SetOpacity { opacity } => {
                self.renderer.set_opacity(opacity.clamp(0.0, 1.0));
                self.needs_redraw = true;
                ControlResponse::Ok
            }

            ControlRequest::SetPaneStatus { status, target } => {
                let pane = match resolve_control_target(&self.runtime.manager, target) {
                    Ok(pane) => pane,
                    Err(response) => return response,
                };
                let status = status.map(|status| status.sanitized());
                let wants_user = status.as_ref().is_some_and(|status| status.attention);
                if let Some(terminal) = self.runtime.manager.terminal_mut(pane) {
                    terminal.status = status;
                }
                if wants_user && !self.window.has_focus() {
                    // Cleared when the window gains focus, as for the bell.
                    self.window.request_user_attention(Some(
                        winit::window::UserAttentionType::Informational,
                    ));
                }
                self.update_tab_bar();
                self.needs_redraw = true;
                ControlResponse::Ok
            }

            ControlRequest::Screenshot { path } => {
                let path = match path {
                    Some(path_str) => std::path::PathBuf::from(path_str),
                    None => {
                        let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
                        let filename = format!("ultty-screenshot-{}.png", timestamp);
                        dirs::picture_dir()
                            .map(|p| p.join("Screenshots").join(&filename))
                            .unwrap_or_else(|| std::path::PathBuf::from(&filename))
                    }
                };
                // Multi-pane tabs capture every pane as the window shows
                // them. This must run before the focused terminal is locked
                // below: the capture locks each pane itself.
                let result = if self.runtime.manager.pane_count() > 1 {
                    Some(self.capture_panes_screenshot(&path))
                } else if let Some(terminal) = self.runtime.manager.focused_terminal() {
                    let term = terminal.event_loop.terminal();
                    let overlay_state = self.build_screenshot_overlay_state();
                    let overlay = overlay_state.as_overlay();

                    // The pane's sessions, as the window draws them: their
                    // surfaces are anchored in this terminal.
                    let native_sessions: Vec<&dyn ultty::scene::view::NativeRenderSource> = self
                        .runtime
                        .protocol
                        .as_ref()
                        .map(|s| {
                            s.pane_or_unbound_sessions(terminal.id)
                                .map(|session| {
                                    session as &dyn ultty::scene::view::NativeRenderSource
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    Some(self.renderer.capture_screenshot_with_native(
                        &term,
                        &overlay,
                        &path,
                        &native_sessions,
                    ))
                } else {
                    None
                };
                match result {
                    Some(Ok(())) => ControlResponse::Data(ControlData::ScreenshotSaved {
                        path: path.display().to_string(),
                    }),
                    Some(Err(e)) => ControlResponse::Error {
                        code: 5,
                        message: format!("Screenshot failed: {}", e),
                    },
                    None => ControlResponse::Error {
                        code: 1,
                        message: "No active terminal".to_string(),
                    },
                }
            }

            ControlRequest::Search {
                pattern,
                regex,
                case_sensitive,
                include_scrollback,
                target,
            } => search_response(
                &self.runtime.manager,
                pattern,
                regex,
                case_sensitive,
                include_scrollback,
                target,
            ),

            ControlRequest::Subscribe { .. } | ControlRequest::SubscribeV2 { .. } => {
                ControlResponse::Error {
                    code: 5,
                    message: "subscribe cannot be used inside batch requests".to_string(),
                }
            }

            ControlRequest::WaitFor { .. } => ControlResponse::Error {
                code: 5,
                message: "wait-for cannot be used inside batch requests".to_string(),
            },

            // Relayed to the app and answered later; see
            // `relay_override_request`. Reached only inside a batch.
            ControlRequest::PushOverride { .. } | ControlRequest::ClearOverride { .. } => {
                ControlResponse::Error {
                    code: 5,
                    message: "override push and clear cannot be used inside batch requests"
                        .to_string(),
                }
            }

            ControlRequest::Batch { commands } => {
                let results: Vec<ControlResponse> = commands
                    .into_iter()
                    .map(|cmd| self.process_control_request(client_key, cmd))
                    .collect();
                ControlResponse::Data(ControlData::BatchResults { results })
            }

            ControlRequest::Authenticate { token } => {
                match self.authenticate_client(client_key, &token) {
                    Ok(()) => ControlResponse::Data(ControlData::Authenticated),
                    Err(response) => response,
                }
            }

            ControlRequest::WithToken { token, request } => {
                // Reached only when nested inside a batch; top-level
                // WithToken is unwrapped in `handle_control_requests`.
                match self.authenticate_client(client_key, &token) {
                    Ok(()) => self.process_control_request(client_key, *request),
                    Err(response) => response,
                }
            }
        }
    }

    /// Check `token` against the token file and mark the client authenticated.
    ///
    /// Fails closed: a missing or unreadable token file rejects every token.
    fn authenticate_client(
        &mut self,
        client_key: usize,
        token: &str,
    ) -> Result<(), ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::ControlResponse;

        let expected = self
            .control_server
            .as_ref()
            .ok_or_else(|| io::Error::other("control server not running"))
            .and_then(|server| read_auth_token(server.token_path()));
        let expected = match expected {
            Ok(expected) => expected,
            Err(e) => {
                tracing::warn!("Control auth token unavailable: {}", e);
                return Err(ControlResponse::Error {
                    code: 500,
                    message: "Control auth token unavailable".to_string(),
                });
            }
        };

        if !token_matches(token, &expected) {
            return Err(ControlResponse::Error {
                code: 403,
                message: "Invalid token".to_string(),
            });
        }

        if let Some(ref mut server) = self.control_server {
            server.set_client_authenticated(client_key, true);
        }
        Ok(())
    }

    /// Process group in the foreground of `pane`'s PTY.
    fn pane_foreground_pgrp(&self, pane: TerminalId) -> Option<i32> {
        self.runtime
            .manager
            .terminal(pane)
            .and_then(|terminal| terminal.event_loop.foreground_pgrp())
    }

    fn forward_key_to_pty(
        &mut self,
        pane: TerminalId,
        key: &ultty_protocol::Key,
        modifiers: ultty_protocol::Modifiers,
    ) -> ultty_protocol::control::ControlResponse {
        let Some(terminal) = self.runtime.manager.terminal_mut(pane) else {
            return input_acceptance(Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the target pane is gone",
            )));
        };

        let ctrl = modifiers.contains(ultty_protocol::Modifiers::CTRL);
        let alt = modifiers.contains(ultty_protocol::Modifiers::ALT);
        let app_cursor = terminal
            .event_loop
            .terminal()
            .modes()
            .application_cursor_keys;

        let bytes = key_to_bytes(key, ctrl, alt, app_cursor);

        if bytes.is_empty() {
            return ultty_protocol::control::ControlResponse::Error {
                code: 4,
                message: "This key has no supported PTY encoding".to_string(),
            };
        }
        input_acceptance(terminal.event_loop.send_input(&bytes))
    }

    fn auth_error_for_request(
        &self,
        client_key: usize,
        request: &ultty_protocol::control::ControlRequest,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::ControlResponse;

        if !control_request_requires_auth(request) {
            return None;
        }

        let authenticated = self
            .control_server
            .as_ref()
            .is_some_and(|server| server.is_client_authenticated(client_key));

        if authenticated {
            None
        } else {
            Some(ControlResponse::Error {
                code: 403,
                message: "Authentication required".to_string(),
            })
        }
    }

    fn find_wait_match(&self, pane: TerminalId, matcher: &WaitMatcher) -> Option<(usize, usize)> {
        let terminal = self.runtime.manager.terminal(pane)?;
        let term = terminal.event_loop.terminal();
        let buffer = term.buffer();
        let total_lines = buffer.total_lines();

        for row in 0..total_lines {
            let Some(grid_row) = buffer.row_at(row) else {
                continue;
            };
            let text = grid_row.text_trimmed();

            if let Some(col) = matcher.find_in(&text) {
                return Some((row, col));
            }
        }

        None
    }
}

/// Error code for a `target` that names no open pane.
const PANE_NOT_FOUND: u16 = 6;
/// No native app to push an override into (or one that cannot take it).
const NO_NATIVE_APP: u16 = 9;
/// The app refused the pushed override, or did not answer in time.
const OVERRIDE_REJECTED: u16 = 10;
/// The recipient has gone or closed its input.
#[cfg(test)]
const INPUT_UNAVAILABLE: u16 = 11;
/// The recipient's bounded input queue cannot accept more bytes.
const INPUT_QUEUE_FULL: u16 = 12;
/// How long an app has to answer `SetOverride`.
const OVERRIDE_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// An override push relayed to a native app, waiting for its answer.
pub(super) struct PendingOverridePush {
    client_key: usize,
    session_key: usize,
    request_id: u32,
    expires: Instant,
}

/// Maximum application-acknowledgement delay; expiry does not undo an action.
const ACTION_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// Total unresolved action acknowledgements, including accessibility.
const MAX_PENDING_CONTROL_ACTIONS: usize = 128;
/// The application refused an action, its target was stale, or its outcome is unknown.
const ACTION_REJECTED: u16 = 15;

struct SemanticActionTarget {
    pane: u64,
    session: u64,
    generation: u64,
    region: u32,
    widget_id: String,
    action: ultty_protocol::SemanticActionKind,
}

/// One application acknowledgement awaited by control or accessibility.
pub(super) struct PendingSemanticAction {
    client_key: Option<usize>,
    session_id: u64,
    request_id: u32,
    expires: Instant,
}

impl PendingSemanticAction {
    fn response(
        &self,
        results: &[(u64, u32, Option<String>)],
        connected: bool,
        now: Instant,
    ) -> Option<ultty_protocol::control::ControlResponse> {
        use ultty_protocol::control::{ControlData, ControlResponse};
        if let Some((_, _, error)) = results.iter().find(|(session, request, _)| {
            *session == self.session_id && *request == self.request_id
        }) {
            return Some(match error {
                None => ControlResponse::Data(ControlData::ActionApplied {
                    session: self.session_id,
                    request_id: self.request_id,
                }),
                Some(error) => ControlResponse::Error {
                    code: ACTION_REJECTED,
                    message: format!("Application refused semantic action: {error}"),
                },
            });
        }
        if !connected || now >= self.expires {
            return Some(ControlResponse::Error {
                code: ACTION_REJECTED,
                message: "Semantic action acknowledgement unavailable; the application may have applied it. Reinspect before retrying".into(),
            });
        }
        None
    }
}

/// Why no session can take an override for a pane.
#[derive(Debug, PartialEq, Eq)]
enum NoOverrideTarget {
    /// No session is bound to the pane, and none of the tab's `unbound`
    /// sessions is in the pane's foreground group.
    NoneForPane { unbound: usize },
    /// The session did not negotiate `CAP_OVERRIDE_PUSH`.
    CannotTakePushes,
    /// The session already has `MAX_AWAITED_OVERRIDE_RESULTS` pushes
    /// unanswered.
    TooManyOutstanding,
}

/// Of the sessions `in_tab` (a pane's tab), the key of the one an
/// override for `pane` goes to: the session bound to the pane (ADR 0025),
/// or, when none is, an unbound session whose process is in the pane's
/// `foreground` group (a background job the binding has not placed yet),
/// which is what `regions --pane` lists for the pane. A session bound to
/// another pane is never picked, so the shell's pane of a split tab is
/// refused rather than pushing into its neighbour's app.
fn pick_override_session(
    in_tab: &[(usize, &ultty::protocol::NativeSession)],
    pane: TerminalId,
    foreground: Option<i32>,
) -> Result<usize, NoOverrideTarget> {
    let bound = in_tab.iter().find(|(_, s)| s.pane() == Some(pane));
    let unbound: Vec<&(usize, &ultty::protocol::NativeSession)> =
        in_tab.iter().filter(|(_, s)| s.pane().is_none()).collect();
    let (key, session) = bound
        .or_else(|| {
            unbound
                .iter()
                .copied()
                .find(|(_, s)| s.is_in_foreground(foreground))
        })
        .ok_or(NoOverrideTarget::NoneForPane {
            unbound: unbound.len(),
        })?;
    if !session.accepts_override_push() {
        return Err(NoOverrideTarget::CannotTakePushes);
    }
    if session.awaiting_override_results() >= ultty::protocol::MAX_AWAITED_OVERRIDE_RESULTS {
        return Err(NoOverrideTarget::TooManyOutstanding);
    }
    Ok(*key)
}

fn is_override_request(request: &ultty_protocol::control::ControlRequest) -> bool {
    use ultty_protocol::control::ControlRequest;
    matches!(
        request,
        ControlRequest::PushOverride { .. } | ControlRequest::ClearOverride { .. }
    )
}

/// Error code for `last_command` in a pane whose shell marked no command.
const NO_COMMAND_RECORDED: u16 = 7;

/// The reply to a resize answered at once: `result` is what
/// `request_inner_size` returned, `current` the window's size after it.
/// `None` means the platform resizes later, which only a top-level request
/// waits for (`request_window_resize`); a batch reports it.
fn resize_control_response(
    result: Option<PhysicalSize<u32>>,
    requested: PhysicalSize<u32>,
    current: PhysicalSize<u32>,
) -> ultty_protocol::control::ControlResponse {
    match (result, current == requested) {
        (_, true) => ultty_protocol::control::ControlResponse::Ok,
        (Some(accepted), false) => ultty_protocol::control::ControlResponse::Error {
            code: RESIZE_NOT_APPLIED,
            message: format!(
                "Window resize request not applied by platform/compositor: requested {}x{}, accepted {}x{}, current {}x{}",
                requested.width,
                requested.height,
                accepted.width,
                accepted.height,
                current.width,
                current.height
            ),
        },
        (None, false) => ultty_protocol::control::ControlResponse::Error {
            code: RESIZE_NOT_APPLIED,
            message: format!(
                "Window resize to {}x{} requested, but the platform applies it later and a batch cannot wait for it; send resize on its own",
                requested.width, requested.height
            ),
        },
    }
}

fn invalid_spawn_options() -> ultty_protocol::control::ControlResponse {
    ultty_protocol::control::ControlResponse::Error {
        code: 4,
        message: "args and working_dir require an explicit command".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semantic_acknowledgement_requires_exact_session_and_request() {
        use ultty_protocol::control::{ControlData, ControlResponse};
        let now = Instant::now();
        let pending = PendingSemanticAction {
            client_key: Some(1),
            session_id: 7,
            request_id: 3,
            expires: now + Duration::from_secs(5),
        };
        assert!(pending.response(&[], true, now).is_none());
        assert!(pending
            .response(&[(8, 3, None), (7, 4, None)], true, now)
            .is_none());
        assert_eq!(
            pending.response(&[(7, 3, None)], true, now),
            Some(ControlResponse::Data(ControlData::ActionApplied {
                session: 7,
                request_id: 3
            }))
        );
        assert!(
            matches!(pending.response(&[(7, 3, Some("stale application frame".into()))], true, now), Some(ControlResponse::Error { code: ACTION_REJECTED, message }) if message.contains("stale application frame"))
        );
        for (connected, instant) in [(false, now), (true, pending.expires)] {
            assert!(
                matches!(pending.response(&[], connected, instant), Some(ControlResponse::Error { code: ACTION_REJECTED, message }) if message.contains("may have applied"))
            );
        }
        let accessibility = PendingSemanticAction {
            client_key: None,
            ..pending
        };
        assert_eq!(
            accessibility.response(&[(7, 3, None)], true, now),
            Some(ControlResponse::Data(ControlData::ActionApplied {
                session: 7,
                request_id: 3
            }))
        );
    }

    #[test]
    fn test_direct_job_response_retains_exit_after_pane_removal() {
        use ultty::terminal_manager::{no_tab_env, TerminalManager};
        use ultty_protocol::control::{ControlData, ControlResponse, ProcessJobOutcome};
        let mut manager = TerminalManager::with_command_tab(
            80,
            24,
            "/bin/sleep",
            &["10".into()],
            &mut no_tab_env,
        )
        .unwrap();
        let tab = manager
            .create_tab_with_command(
                "/bin/sh",
                &["-c".into(), "exit 7".into()],
                None,
                None,
                &mut no_tab_env,
            )
            .unwrap();
        let job = manager
            .tabs()
            .iter()
            .find(|candidate| candidate.id == tab)
            .unwrap()
            .focused_terminal;
        let deadline = Instant::now() + Duration::from_secs(3);
        while job_is_running(&process_job_response(&manager, job)) {
            assert!(Instant::now() < deadline);
            manager.process_all().unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            process_job_response(&manager, job),
            ControlResponse::Data(ControlData::ProcessJob {
                outcome: ProcessJobOutcome::Exited { status: 7 },
                ..
            })
        ));
        manager.close_tab(tab);
        assert!(manager.terminal(job).is_none());
        assert!(matches!(
            process_job_response(&manager, job),
            ControlResponse::Data(ControlData::ProcessJob {
                outcome: ProcessJobOutcome::Exited { status: 7 },
                ..
            })
        ));
        assert!(matches!(
            process_job_response(&manager, TerminalId::new(usize::MAX)),
            ControlResponse::Error { code: 404, .. }
        ));
    }

    #[test]
    fn test_program_status_query_preserves_records_and_generation() {
        use ultty::terminal_manager::{no_tab_env, TerminalManager};
        use ultty_protocol::control::ControlResponse;

        let manager = TerminalManager::with_command_tab(
            80,
            24,
            "/bin/sleep",
            &["10".to_string()],
            &mut no_tab_env,
        )
        .unwrap();
        let pane = manager.focused_terminal().unwrap().id;
        manager.terminal(pane).unwrap().event_loop.write_local(
            b"\x1b]7501;state=blocked:id=task:app=build:kind=permission:progress=25:msg=QXBwcm92ZT8=\x1b\\"
        );
        let source = manager
            .terminal(pane)
            .unwrap()
            .event_loop
            .terminal()
            .program_status_snapshot();
        let response = program_status_response(&manager, pane);
        let ControlResponse::Data(ultty_protocol::control::ControlData::ProgramStatus {
            generation,
            reason,
            records,
        }) = response
        else {
            panic!("expected a program status snapshot");
        };
        let encoded =
            serde_json::json!({ "generation": generation, "reason": reason, "records": records });
        assert_eq!(encoded, serde_json::to_value(source).unwrap());
        assert_eq!(encoded["records"][0]["msg"], "Approve?");
    }

    #[test]
    fn test_control_shell_submission_observes_real_fast_completion() {
        use ultty::terminal_manager::{no_tab_env, TerminalManager};
        use ultty_protocol::control::{ControlData, ControlResponse};

        let script = "printf '\\033]133;A\\007\\033]133;B\\007'; IFS= read -r line; printf '\\033]133;C\\007'; eval \"$line\"; code=$?; printf '\\033]133;D;%s\\007' \"$code\"";
        let mut manager = TerminalManager::with_command_tab(
            80,
            24,
            "/bin/sh",
            &["-c".to_string(), script.to_string()],
            &mut no_tab_env,
        )
        .unwrap();
        let pane = manager.focused_terminal().unwrap().id;
        let deadline = Instant::now() + Duration::from_secs(5);
        let sequence = loop {
            match submit_control_shell_command(&mut manager, pane, "false") {
                Ok(sequence) => break sequence,
                Err(ControlResponse::Error {
                    code: INPUT_QUEUE_FULL,
                    ..
                }) => {}
                Err(response) => panic!("submission failed: {response:?}"),
            }
            manager
                .terminal_mut(pane)
                .unwrap()
                .event_loop
                .wait(Duration::from_millis(10))
                .unwrap();
            assert!(
                Instant::now() < deadline,
                "shell did not show an idle prompt"
            );
        };
        let outcome = loop {
            if let Some(outcome) = manager
                .terminal(pane)
                .unwrap()
                .event_loop
                .shell_command_result(sequence)
            {
                break outcome;
            }
            manager
                .terminal_mut(pane)
                .unwrap()
                .event_loop
                .wait(Duration::from_millis(10))
                .unwrap();
            assert!(Instant::now() < deadline, "shell did not report completion");
        };
        assert_eq!(
            shell_command_response(pane, sequence, outcome),
            ControlResponse::Data(ControlData::ShellCommandFinished {
                pane: pane.raw() as u64,
                sequence,
                exit_code: Some(1)
            })
        );
    }

    #[test]
    fn test_pty_control_input_reports_queue_acceptance_and_refusal() {
        use ultty::terminal_manager::{no_tab_env, TerminalManager};
        use ultty_protocol::control::ControlResponse;

        let mut manager = TerminalManager::with_command_tab(
            80,
            24,
            "/bin/sleep",
            &["10".to_string()],
            &mut no_tab_env,
        )
        .unwrap();
        let pane = manager.focused_terminal().unwrap().id;
        assert_eq!(
            send_pty_input(&mut manager, pane, b"small"),
            ControlResponse::Ok
        );
        let oversized = vec![b'x'; 16 * 1024 * 1024 + 1];
        assert!(matches!(
            send_pty_input(&mut manager, pane, &oversized),
            ControlResponse::Error {
                code: INPUT_QUEUE_FULL,
                ..
            }
        ));
        assert!(matches!(
            send_pty_input(&mut manager, TerminalId::new(999), b"x"),
            ControlResponse::Error {
                code: INPUT_UNAVAILABLE,
                ..
            }
        ));
    }

    #[test]
    fn test_pty_control_input_rejects_a_closed_child() {
        use ultty::event_loop::TerminalEvent;
        use ultty::terminal_manager::{no_tab_env, TerminalManager};
        use ultty_protocol::control::ControlResponse;

        let mut manager = TerminalManager::with_command_tab(
            80,
            24,
            "/bin/sh",
            &["-c".to_string(), "printf finished".to_string()],
            &mut no_tab_env,
        )
        .unwrap();
        let pane = manager.focused_terminal().unwrap().id;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events = manager
                .terminal_mut(pane)
                .unwrap()
                .event_loop
                .wait(Duration::from_millis(50))
                .unwrap();
            if events
                .iter()
                .any(|event| matches!(event, TerminalEvent::ChildExited { .. }))
            {
                break;
            }
            assert!(Instant::now() < deadline, "child did not finish");
        }
        assert!(matches!(
            send_pty_input(&mut manager, pane, b"too late"),
            ControlResponse::Error {
                code: INPUT_UNAVAILABLE,
                ..
            }
        ));
    }

    /// A 6x2 buffer with five lines written: three in history, two on screen.
    fn buffer_with_history() -> ultty::grid::Buffer {
        let mut buffer = ultty::grid::Buffer::new(ultty::grid::Dimensions::new(6, 2));
        for (i, line) in ["one", "two", "three", "four", "five"].iter().enumerate() {
            buffer.write_ascii_span(line.as_bytes());
            if i < 4 {
                buffer.grid_mut().carriage_return();
                buffer.newline();
            }
        }
        buffer
    }

    fn lines(text: &str) -> Vec<&str> {
        text.lines().map(str::trim_end).collect()
    }

    /// A socket session whose peer is this process, so its group is ours,
    /// after a hello asking for `capabilities`. The peer end is returned so
    /// the socket stays open.
    fn own_session(
        session_id: u64,
        capabilities: u32,
    ) -> (
        ultty::protocol::NativeSession,
        std::os::unix::net::UnixStream,
    ) {
        use std::io::Write;
        use ultty_protocol::Encode;
        let (ours, mut theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut session = ultty::protocol::NativeSession::new(
            ours,
            session_id,
            ultty::terminal_manager::TabId::new(0),
        )
        .unwrap();
        theirs
            .write_all(
                &ultty_protocol::AppMessage::ClientHello {
                    version: ultty_protocol::PROTOCOL_VERSION,
                    requested_capabilities: capabilities,
                }
                .encode_to_bytes(),
            )
            .unwrap();
        assert!(session.poll().unwrap());
        (session, theirs)
    }

    /// An override goes to the session bound to the pane (ADR 0025); a
    /// session bound to another pane is never picked, however the
    /// foreground falls, so the shell's pane of a split tab is refused. An
    /// unbound session is reached through the foreground rule only.
    #[test]
    fn override_target_is_the_panes_bound_session_or_an_unbound_foreground_one() {
        use ultty::terminal_manager::TerminalId;
        let ours = ultty::platform::socket::process_group(std::process::id() as i32).unwrap();
        let app_pane = TerminalId::new(1);
        let shell_pane = TerminalId::new(2);
        let (mut bound, _peer) = own_session(1, ultty_protocol::CAP_OVERRIDE_PUSH);
        bound.bind_pane(app_pane);
        let (unbound, _peer2) = own_session(2, ultty_protocol::CAP_OVERRIDE_PUSH);
        let both = [(5usize, &bound), (6usize, &unbound)];

        // The bound session wins for its pane whatever the foreground is.
        assert_eq!(
            pick_override_session(&both, app_pane, Some(ours + 1)),
            Ok(5)
        );
        // Another pane gets an unbound session only through the foreground.
        assert_eq!(pick_override_session(&both, shell_pane, Some(ours)), Ok(6));
        assert_eq!(
            pick_override_session(&both, shell_pane, Some(ours + 1)),
            Err(NoOverrideTarget::NoneForPane { unbound: 1 })
        );
        // With every session bound elsewhere, the shell's pane has none.
        let only_bound = [(5usize, &bound)];
        assert_eq!(
            pick_override_session(&only_bound, shell_pane, Some(ours)),
            Err(NoOverrideTarget::NoneForPane { unbound: 0 }),
            "a session bound to the next pane must not be picked"
        );
        assert_eq!(
            pick_override_session(&[], app_pane, Some(ours)),
            Err(NoOverrideTarget::NoneForPane { unbound: 0 })
        );

        let (mut raw, _peer3) = own_session(3, ultty_protocol::CLIENT_DEFAULT_CAPABILITIES);
        raw.bind_pane(app_pane);
        assert_eq!(
            pick_override_session(&[(7, &raw)], app_pane, None),
            Err(NoOverrideTarget::CannotTakePushes)
        );

        // A session with the most pushes unanswered takes no more.
        for request_id in 0..ultty::protocol::MAX_AWAITED_OVERRIDE_RESULTS as u32 {
            bound.send_set_override(request_id, None).unwrap();
        }
        assert_eq!(
            pick_override_session(&[(5, &bound)], app_pane, None),
            Err(NoOverrideTarget::TooManyOutstanding)
        );
    }

    #[test]
    fn text_without_scrollback_is_the_screen() {
        let buffer = buffer_with_history();
        assert_eq!(buffer.history_len(), 3);
        assert_eq!(lines(&buffer_text(&buffer, 0)), ["four", "five"]);
    }

    #[test]
    fn text_with_scrollback_includes_history_oldest_first() {
        let buffer = buffer_with_history();
        assert_eq!(
            lines(&buffer_text(&buffer, usize::MAX)),
            ["one", "two", "three", "four", "five"]
        );
    }

    #[test]
    fn text_with_a_scrollback_limit_keeps_the_most_recent_lines() {
        let buffer = buffer_with_history();
        assert_eq!(lines(&buffer_text(&buffer, 1)), ["three", "four", "five"]);
    }
    use ultty_protocol::control::{ControlRequest, ControlResponse};
    use ultty_protocol::Key;

    // ---------------------------------------------------------------
    // key_to_bytes: simple keys (no modifiers, no app-cursor)
    // ---------------------------------------------------------------

    #[test]
    fn mutating_control_requests_require_auth() {
        assert!(control_request_requires_auth(&ControlRequest::SendText {
            text: "hello".to_string(),
            target: None,
        }));
        assert!(control_request_requires_auth(&ControlRequest::NewTab {
            title: None,
            command: None,
            args: Vec::new(),
            working_dir: None,
        }));
        assert!(control_request_requires_auth(&ControlRequest::RunCommand {
            command: "make".to_string(),
            args: vec!["test".to_string()],
            target: None,
        }));
        assert!(control_request_requires_auth(&ControlRequest::SplitPane {
            direction: ultty_protocol::control::SplitDirection::Right,
            command: None,
            args: Vec::new(),
            working_dir: None,
            target: None,
        }));
    }

    #[test]
    fn screen_reading_control_requests_require_auth() {
        assert!(control_request_requires_auth(&ControlRequest::GetText {
            include_scrollback: true,
            scrollback_lines: None,
            target: None,
        }));
        assert!(control_request_requires_auth(
            &ControlRequest::GetSelection { target: None }
        ));
        assert!(control_request_requires_auth(&ControlRequest::Search {
            pattern: "needle".to_string(),
            regex: false,
            case_sensitive: false,
            include_scrollback: true,
            target: None,
        }));
        assert!(control_request_requires_auth(&ControlRequest::WaitFor {
            pattern: "ready".to_string(),
            regex: false,
            timeout_ms: 100,
            until: Default::default(),
            target: None,
        }));
        assert!(control_request_requires_auth(&ControlRequest::GetInfo));
        assert!(control_request_requires_auth(&ControlRequest::ListPanes));
    }

    #[test]
    fn only_probes_and_auth_are_exempt_from_auth() {
        assert!(!control_request_requires_auth(&ControlRequest::Ping));
        assert!(!control_request_requires_auth(&ControlRequest::Version));
        assert!(!control_request_requires_auth(
            &ControlRequest::Authenticate {
                token: "t".to_string(),
            }
        ));
        assert!(!control_request_requires_auth(&ControlRequest::WithToken {
            token: "t".to_string(),
            request: Box::new(ControlRequest::Ping),
        }));
    }

    #[test]
    fn write_auth_token_creates_private_random_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("ultty-auth.token");

        let first = write_auth_token(&path).unwrap();
        let second = write_auth_token(&path).unwrap();

        assert_eq!(first.len(), AUTH_TOKEN_BYTES * 2);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second, "token must be regenerated");
        assert_eq!(read_auth_token(&path).unwrap(), second);

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn read_auth_token_fails_closed_when_missing_or_empty() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.token");
        assert!(read_auth_token(&missing).is_err());

        let empty = dir.path().join("empty.token");
        std::fs::write(&empty, "\n").unwrap();
        assert!(read_auth_token(&empty).is_err());
    }

    #[test]
    fn token_matches_ignores_surrounding_whitespace_only() {
        assert!(token_matches("abc123\n", "abc123"));
        assert!(!token_matches("abc124", "abc123"));
        assert!(!token_matches("abc12", "abc123"));
        assert!(!token_matches("", "abc123"));
    }

    #[test]
    fn plain_search_match_uses_char_columns_on_multibyte_rows() {
        let text = "aİé ready";
        let pattern_lower = "ready";
        let found: Vec<_> = find_ignore_case(text, pattern_lower).collect();
        assert_eq!(found.len(), 1);
        let m = found[0];
        let result = plain_search_match(text, 3, m.char_col, m.byte_start, m.byte_end);

        assert_eq!(result.line, 3);
        assert_eq!(result.col, 4);
        assert_eq!(result.text, "ready");
        assert_eq!(result.context_before, "aİé ");
        assert_eq!(result.context_after, "");
    }

    #[test]
    fn info_does_not_lock_the_terminal_twice() {
        // Debug builds panic on a re-entrant terminal lock (release builds
        // deadlock), so this fails if terminal_info holds the lock while
        // looking up the working directory.
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        manager
            .focused_terminal_mut()
            .unwrap()
            .event_loop
            .terminal_mut()
            .set_working_directory("/tmp/ultty-info-cwd".to_string());
        let info = terminal_info(manager.focused_terminal().unwrap(), 0, 1);
        assert_eq!((info.cols, info.rows), (80, 24));
        assert_eq!(info.cwd.as_deref(), Some("/tmp/ultty-info-cwd"));
    }

    fn pane_ids(panes: &[ultty_protocol::control::PaneInfo]) -> Vec<(u64, usize)> {
        panes.iter().map(|pane| (pane.id, pane.tab_index)).collect()
    }

    fn focused_pane(panes: &[ultty_protocol::control::PaneInfo]) -> Vec<u64> {
        panes
            .iter()
            .filter(|pane| pane.focused)
            .map(|pane| pane.id)
            .collect()
    }

    #[test]
    fn panes_list_every_pane_in_layout_order_with_stable_ids() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        let left = manager.focused_terminal_id().unwrap().raw() as u64;
        let right = manager.split_horizontal().unwrap().unwrap().raw() as u64;
        manager.create_tab().unwrap();
        let other = manager.focused_terminal_id().unwrap().raw() as u64;

        let panes = pane_infos(&manager);
        assert_eq!(pane_ids(&panes), vec![(left, 0), (right, 0), (other, 1)]);
        // Only the active pane of the active tab has keyboard focus; each tab
        // still reports its own active pane (a split keeps focus where it was).
        assert_eq!(focused_pane(&panes), vec![other]);
        let active: Vec<u64> = panes.iter().filter(|p| p.active).map(|p| p.id).collect();
        assert_eq!(active, vec![left, other]);
        assert!(panes
            .iter()
            .all(|p| p.pid.is_some() && p.cols > 0 && p.rows > 0));

        // Switching tabs, moving focus and reordering tabs move the focus
        // and the tab index, never the ids.
        manager.switch_to_tab(0);
        manager.focus_next_pane();
        assert_eq!(focused_pane(&pane_infos(&manager)), vec![right]);
        assert!(manager.reorder_tabs(0, 1));
        assert_eq!(
            pane_ids(&pane_infos(&manager)),
            vec![(other, 0), (left, 1), (right, 1)]
        );
    }

    #[test]
    fn target_resolves_to_the_named_pane_or_the_focused_one() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        let focused = manager.focused_terminal_id().unwrap();
        let other = manager.split_horizontal().unwrap().unwrap();

        assert_eq!(resolve_control_target(&manager, None), Ok(focused));
        assert_eq!(
            resolve_control_target(&manager, Some(other.raw() as u64)),
            Ok(other)
        );
    }

    #[test]
    fn target_naming_no_open_pane_is_an_error() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        let closed = manager.split_horizontal().unwrap().unwrap();
        manager.focus_terminal(closed);
        assert!(manager.close_focused_pane());

        for target in [closed.raw() as u64, 999, u64::MAX] {
            match resolve_control_target(&manager, Some(target)) {
                Err(ultty_protocol::control::ControlResponse::Error { code, message }) => {
                    assert_eq!(code, PANE_NOT_FOUND);
                    assert!(message.contains(&target.to_string()), "{message}");
                }
                other => panic!("target {target}: expected an error, got {other:?}"),
            }
        }
    }

    #[test]
    fn closed_pane_ids_are_not_reused() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        let split = manager.split_horizontal().unwrap().unwrap();
        let closed = split.raw() as u64;
        manager.focus_terminal(split);
        assert!(manager.close_focused_pane());
        let next = manager.split_horizontal().unwrap().unwrap().raw() as u64;
        let ids: Vec<u64> = pane_infos(&manager).iter().map(|p| p.id).collect();
        assert!(!ids.contains(&closed));
        assert!(ids.contains(&next));
    }

    #[test]
    fn terminal_working_directory_prefers_terminal_reported_cwd() {
        let mut manager = ultty::terminal_manager::TerminalManager::new(80, 24).unwrap();
        manager
            .focused_terminal_mut()
            .unwrap()
            .event_loop
            .terminal_mut()
            .set_working_directory("/tmp/ultty-control-cwd".to_string());

        assert_eq!(
            terminal_working_directory(manager.focused_terminal().unwrap()).as_deref(),
            Some("/tmp/ultty-control-cwd")
        );
    }

    #[test]
    fn key_char_plain_ascii() {
        assert_eq!(key_to_bytes(&Key::Char('a'), false, false, false), b"a");
        assert_eq!(key_to_bytes(&Key::Char('Z'), false, false, false), b"Z");
        assert_eq!(key_to_bytes(&Key::Char('5'), false, false, false), b"5");
    }

    #[test]
    fn resize_control_response_in_a_batch_reports_a_resize_still_to_come() {
        let response = resize_control_response(
            None,
            PhysicalSize::new(1000, 700),
            PhysicalSize::new(800, 600),
        );

        assert!(matches!(
            response,
            ControlResponse::Error {
                code: 8,
                ref message
            } if message.contains("1000x700") && message.contains("batch")
        ));
    }

    #[test]
    fn resize_control_response_reports_ok_when_a_later_resize_already_applied() {
        let response = resize_control_response(
            None,
            PhysicalSize::new(800, 600),
            PhysicalSize::new(800, 600),
        );

        assert!(matches!(response, ControlResponse::Ok));
    }

    #[test]
    fn resize_control_response_reports_ok_when_platform_accepts_request() {
        let response = resize_control_response(
            Some(PhysicalSize::new(800, 600)),
            PhysicalSize::new(800, 600),
            PhysicalSize::new(800, 600),
        );

        assert!(matches!(response, ControlResponse::Ok));
    }

    #[test]
    fn resize_control_response_reports_adjusted_size() {
        let response = resize_control_response(
            Some(PhysicalSize::new(968, 709)),
            PhysicalSize::new(1280, 720),
            PhysicalSize::new(968, 709),
        );

        assert!(matches!(
            response,
            ControlResponse::Error {
                code: 8,
                ref message
            } if message.contains("not applied")
                && message.contains("1280x720")
                && message.contains("968x709")
        ));
    }

    #[test]
    fn resize_control_response_reports_unapplied_even_when_platform_echoes_request() {
        let response = resize_control_response(
            Some(PhysicalSize::new(1280, 720)),
            PhysicalSize::new(1280, 720),
            PhysicalSize::new(800, 600),
        );

        assert!(matches!(
            response,
            ControlResponse::Error {
                code: 8,
                ref message
            } if message.contains("not applied")
                && message.contains("current 800x600")
        ));
    }

    #[test]
    fn key_char_multibyte_utf8() {
        // A non-ASCII character should encode as its UTF-8 bytes.
        let bytes = key_to_bytes(&Key::Char('\u{00e9}'), false, false, false); // 'e' with accent
        assert_eq!(bytes, "\u{00e9}".as_bytes());

        let bytes = key_to_bytes(&Key::Char('\u{4e2d}'), false, false, false); // CJK character
        assert_eq!(bytes, "\u{4e2d}".as_bytes());
    }

    #[test]
    fn key_enter() {
        assert_eq!(key_to_bytes(&Key::Enter, false, false, false), b"\r");
    }

    #[test]
    fn key_tab() {
        assert_eq!(key_to_bytes(&Key::Tab, false, false, false), b"\t");
    }

    #[test]
    fn key_escape() {
        assert_eq!(key_to_bytes(&Key::Escape, false, false, false), b"\x1b");
    }

    #[test]
    fn key_backspace() {
        assert_eq!(key_to_bytes(&Key::Backspace, false, false, false), b"\x7f");
    }

    #[test]
    fn key_delete() {
        assert_eq!(key_to_bytes(&Key::Delete, false, false, false), b"\x1b[3~");
    }

    #[test]
    fn key_insert() {
        assert_eq!(key_to_bytes(&Key::Insert, false, false, false), b"\x1b[2~");
    }

    #[test]
    fn key_page_up_down() {
        assert_eq!(key_to_bytes(&Key::PageUp, false, false, false), b"\x1b[5~");
        assert_eq!(
            key_to_bytes(&Key::PageDown, false, false, false),
            b"\x1b[6~"
        );
    }

    // ---------------------------------------------------------------
    // key_to_bytes: ctrl modifier
    // ---------------------------------------------------------------

    #[test]
    fn key_ctrl_a_through_z() {
        // Ctrl-A = 0x01, Ctrl-Z = 0x1A
        assert_eq!(key_to_bytes(&Key::Char('a'), true, false, false), vec![1]);
        assert_eq!(key_to_bytes(&Key::Char('c'), true, false, false), vec![3]);
        assert_eq!(key_to_bytes(&Key::Char('z'), true, false, false), vec![26]);
    }

    #[test]
    fn key_ctrl_uppercase_is_same_as_lowercase() {
        // Ctrl-A and Ctrl-a should produce the same byte.
        assert_eq!(
            key_to_bytes(&Key::Char('A'), true, false, false),
            key_to_bytes(&Key::Char('a'), true, false, false)
        );
        assert_eq!(
            key_to_bytes(&Key::Char('C'), true, false, false),
            key_to_bytes(&Key::Char('c'), true, false, false)
        );
    }

    #[test]
    fn key_ctrl_non_alpha_passes_through() {
        // Ctrl + non-alpha char should not produce a control character;
        // it falls through to the alt/plain branch.
        assert_eq!(key_to_bytes(&Key::Char('5'), true, false, false), b"5");
    }

    // ---------------------------------------------------------------
    // key_to_bytes: alt modifier
    // ---------------------------------------------------------------

    #[test]
    fn key_alt_char() {
        // Alt-a should be ESC followed by 'a'
        assert_eq!(
            key_to_bytes(&Key::Char('a'), false, true, false),
            vec![0x1b, b'a']
        );
        assert_eq!(
            key_to_bytes(&Key::Char('x'), false, true, false),
            vec![0x1b, b'x']
        );
    }

    #[test]
    fn key_alt_multibyte_char() {
        // Alt + non-ASCII should be ESC + UTF-8 bytes
        let bytes = key_to_bytes(&Key::Char('\u{00e9}'), false, true, false);
        let mut expected = vec![0x1b];
        expected.extend_from_slice("\u{00e9}".as_bytes());
        assert_eq!(bytes, expected);
    }

    #[test]
    fn key_ctrl_takes_precedence_over_alt_for_alpha() {
        // When both ctrl and alt are set for an alpha char, ctrl wins.
        assert_eq!(key_to_bytes(&Key::Char('c'), true, true, false), vec![3]);
    }

    // ---------------------------------------------------------------
    // key_to_bytes: arrow keys with and without app-cursor mode
    // ---------------------------------------------------------------

    #[test]
    fn arrow_keys_normal_mode() {
        assert_eq!(key_to_bytes(&Key::Up, false, false, false), b"\x1b[A");
        assert_eq!(key_to_bytes(&Key::Down, false, false, false), b"\x1b[B");
        assert_eq!(key_to_bytes(&Key::Right, false, false, false), b"\x1b[C");
        assert_eq!(key_to_bytes(&Key::Left, false, false, false), b"\x1b[D");
    }

    #[test]
    fn arrow_keys_app_cursor_mode() {
        assert_eq!(key_to_bytes(&Key::Up, false, false, true), b"\x1bOA");
        assert_eq!(key_to_bytes(&Key::Down, false, false, true), b"\x1bOB");
        assert_eq!(key_to_bytes(&Key::Right, false, false, true), b"\x1bOC");
        assert_eq!(key_to_bytes(&Key::Left, false, false, true), b"\x1bOD");
    }

    #[test]
    fn home_end_normal_mode() {
        assert_eq!(key_to_bytes(&Key::Home, false, false, false), b"\x1b[H");
        assert_eq!(key_to_bytes(&Key::End, false, false, false), b"\x1b[F");
    }

    #[test]
    fn home_end_app_cursor_mode() {
        assert_eq!(key_to_bytes(&Key::Home, false, false, true), b"\x1bOH");
        assert_eq!(key_to_bytes(&Key::End, false, false, true), b"\x1bOF");
    }

    // ---------------------------------------------------------------
    // key_to_bytes: function keys
    // ---------------------------------------------------------------

    #[test]
    fn function_keys_f1_through_f4() {
        assert_eq!(
            key_to_bytes(&Key::Function(1), false, false, false),
            b"\x1bOP"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(2), false, false, false),
            b"\x1bOQ"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(3), false, false, false),
            b"\x1bOR"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(4), false, false, false),
            b"\x1bOS"
        );
    }

    #[test]
    fn function_keys_f5_through_f12() {
        assert_eq!(
            key_to_bytes(&Key::Function(5), false, false, false),
            b"\x1b[15~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(6), false, false, false),
            b"\x1b[17~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(7), false, false, false),
            b"\x1b[18~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(8), false, false, false),
            b"\x1b[19~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(9), false, false, false),
            b"\x1b[20~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(10), false, false, false),
            b"\x1b[21~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(11), false, false, false),
            b"\x1b[23~"
        );
        assert_eq!(
            key_to_bytes(&Key::Function(12), false, false, false),
            b"\x1b[24~"
        );
    }

    #[test]
    fn function_key_out_of_range_returns_empty() {
        assert_eq!(
            key_to_bytes(&Key::Function(0), false, false, false),
            Vec::<u8>::new()
        );
        assert_eq!(
            key_to_bytes(&Key::Function(13), false, false, false),
            Vec::<u8>::new()
        );
        assert_eq!(
            key_to_bytes(&Key::Function(255), false, false, false),
            Vec::<u8>::new()
        );
    }

    // ---------------------------------------------------------------
    // build_command_line
    // ---------------------------------------------------------------

    #[test]
    fn command_line_no_args() {
        assert_eq!(build_command_line("ls", &[]), "ls\n");
    }

    #[test]
    fn command_line_with_args() {
        let args = vec!["--all".to_string(), "-l".to_string()];
        assert_eq!(build_command_line("ls", &args), "ls --all -l\n");
    }

    #[test]
    fn command_line_single_arg() {
        let args = vec!["/tmp".to_string()];
        assert_eq!(build_command_line("cd", &args), "cd /tmp\n");
    }

    #[test]
    fn command_line_always_ends_with_newline() {
        assert!(build_command_line("echo", &[]).ends_with('\n'));
        let args = vec!["hello".to_string()];
        assert!(build_command_line("echo", &args).ends_with('\n'));
    }

    // ---------------------------------------------------------------
    // Protocol type round-trip / construction tests
    // ---------------------------------------------------------------

    #[test]
    fn control_request_ping_serialization_roundtrip() {
        use ultty_protocol::control::ControlRequest;
        let req = ControlRequest::Ping;
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);
    }

    #[test]
    fn control_response_error_has_correct_fields() {
        use ultty_protocol::control::ControlResponse;
        let resp = ControlResponse::Error {
            code: 1,
            message: "No active terminal".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"code\":1"));
        assert!(json.contains("No active terminal"));
    }

    #[test]
    fn control_response_pong_roundtrip() {
        use ultty_protocol::control::{ControlData, ControlResponse};
        let resp = ControlResponse::Data(ControlData::Pong);
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ControlResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, resp);
    }

    #[test]
    fn version_info_construction() {
        use ultty_protocol::control::VersionInfo;
        let info = VersionInfo {
            version: "0.1.0".to_string(),
            protocol_version: 1,
            git_hash: Some("abc123".to_string()),
        };
        assert_eq!(info.version, "0.1.0");
        assert_eq!(info.protocol_version, 1);
        assert_eq!(info.git_hash, Some("abc123".to_string()));
    }

    #[test]
    fn batch_request_serialization_roundtrip() {
        use ultty_protocol::control::ControlRequest;
        let batch = ControlRequest::Batch {
            commands: vec![
                ControlRequest::Ping,
                ControlRequest::Version,
                ControlRequest::GetSelection { target: None },
            ],
        };
        let json = serde_json::to_string(&batch).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, batch);
    }

    #[test]
    fn search_request_serialization_roundtrip() {
        use ultty_protocol::control::ControlRequest;
        let req = ControlRequest::Search {
            pattern: "hello.*world".to_string(),
            regex: true,
            case_sensitive: false,
            include_scrollback: true,
            target: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, req);
    }
}
