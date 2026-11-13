# Native semantic actions

Ulttui applications can expose focus and activation for rendered widgets. Tools
and accessibility clients use this path to invoke the application's handler.
An activation acknowledgement confirms dispatch to the app's message/update
path. It does not confirm completion of asynchronous work that handler starts.

The native session must negotiate `CAP_SEMANTIC_ACTIONS` (`0x1000`). Raw client
defaults exclude it because a client that requests it must answer every action.
ANSI applications continue to use keyboard input. There is no ANSI action API or
additional terminal input reader.

## Application handlers

Focus is available for visible, enabled, uniquely identified focusable widgets.
Activation requires both an explicit declaration and a handler:

```rust
fn supports_action(&self, id: &str, action: SemanticActionKind) -> bool {
    action == SemanticActionKind::Activate && id == "increment"
}

fn on_action(&mut self, id: &str, action: SemanticActionKind) -> Option<Message> {
    match (id, action) {
        ("increment", SemanticActionKind::Activate) => Some(Message::Increment),
        _ => None,
    }
}
```

Use the same message for Enter, a shortcut, and activation. A button role alone
does not declare a handler. Returning `None` refuses the action. The runner also
refuses actions while a screen overlay is active or application updates await
rendering. Screen-specific action handlers can be added through a separate
screen contract later.

Luau apps declare activation IDs in their app table:

```lua
actions = { increment = "activate" },
```

The existing `update` receives `{ type = "action", id = "increment",
action = "activate" }`. Apps without `actions` expose no activation handlers.
See `examples/lua/hello.luau` for shared keyboard/action dispatch.

## Identity and stale requests

Widget IDs come from `View::widget_id()`. Native reconciliation keys come from
`View::native_key()` and may differ. The renderer records them separately. A
hidden ancestor excludes its subtree; a disabled ancestor prevents actions.
Duplicate visible widget IDs expose no actions for any duplicate.

Region metadata declares `ulttui.widget_id` and `ulttui.actions`. The latter is a
comma-separated list containing `focus`, `activate`, both in that order, or an
empty list. The protocol's `declared_actions` helper rejects duplicate keys,
duplicate action names, unknown names, and invalid widget IDs. The existing `id`
metadata continues to hold the native key.

An external observation uses the terminal's `shown_generation`, a `u64`. The
terminal validates it before routing. The native request then carries a distinct
application `FrameId`, a `u32`, plus exact region ID, widget ID, action, and
terminal-owned request ID. The app validates all of those fields against its
last presented registry. Handler declaration or widget identity changes force a
frame even when pixels stay the same.

For sessions negotiating actions, application frame IDs strictly increase and
never repeat. Ulttui refuses frame identity exhaustion before wrap. The terminal
tracks accepted and committed frame IDs separately, rejects reused IDs, and
refuses actions while a frame is open or after unframed scene mutations.
Focus/blur, scrolling, keepalives, and action acknowledgements preserve the
committed registry fence. The app installs its next registry only after writing
the corresponding `EndFrame` successfully.

## Wire results and bounds

Terminal message `InvokeAction`, type `0x0081`, carries `request_id: u32`,
`frame_id: u32`, `region_id: u32`, a length-prefixed widget ID, and action byte
`0` for focus or `1` for activate. Application `ActionResult`, also `0x0081` in
the opposite direction, carries the same request ID and an optional refusal.
An absent refusal means applied dispatch. Requests that expire or disconnect
after being sent have an unknown outcome and must not be retried automatically.

Widget IDs contain 1 through 256 UTF-8 bytes and no control characters. Refusals
are bounded to 1024 UTF-8 bytes without controls and should contain no app data.
The framework retains at most 64 pending requests. A full queue returns a
refusal. The terminal separately bounds outstanding correlation and ignores
duplicate or unknown acknowledgements.

The counter example exposes `inc`, `dec`, and `reset`. The form exposes `submit`
or `reset` according to its displayed state. Its status reporting write result
is separate from form validation and submission success.
