# Design

Ultty is a serious terminal workstation. The interface should feel precise,
quiet, inspectable, and fast. Chrome exists to orient the user and expose state;
it must not compete with terminal content.

## Visual Register

Use a dense systems-tool register with an experimental workstation edge.
Avoid generic SaaS cards, decorative gradients, retro terminal cosplay, neon
cyberpunk styling, and editor-clone chrome. The surface should read as a
native terminal first.

## Chrome

The top chrome has two possible bands:

1. Title/control band: always visible when client-side decoration is enabled.
   It owns window controls, window dragging, and the new-tab button.
2. Tab rail: visible only when two or more tabs exist.

Single-tab state must not show an empty tab rail. It should preserve vertical
space for terminal content.

Multi-tab state shows the tab rail below the title/control band. Tabs must not
share the same visual row as window controls. The active tab should connect to
the terminal surface; inactive tabs should be visible but quiet.

Window controls stay in the upper band. Tab hit testing, tab hover, tab close,
middle-click close, and drag-to-reorder apply only in the lower tab rail.
The new-tab button stays in the upper band in both single-tab and multi-tab
states.

## Terminal Surface

Terminal content is primary. Prompt, cursor, scrollback, and native protocol
content should remain visually dominant over chrome. Borders and separators
should be one-pixel or similarly restrained unless indicating an active mode or
error state.

Scrollbars are secondary orientation aids. They should not resize content or
introduce layout shifts.

## Color

Prefer local contrast steps derived from the active terminal theme over a fixed
brand palette. Use color to clarify hierarchy, not to decorate.

Required state distinctions:

- active tab: foreground text plus a subtle accent edge
- inactive tab: dimmed foreground and lower-contrast fill
- hovered controls: clear enough to target, quieter than active state
- destructive close hover: distinct red tone plus control placement, not color
  alone

Do not rely on color alone for critical state. Target WCAG AA where text is
used as UI.

## Typography

Chrome text should use terminal-scale typography, not hero or dashboard
heading sizes. Labels must fit common narrow widths and truncate before
overlapping close targets.

Use familiar symbols for compact controls where the renderer supports them.
Avoid decorative Unicode blocks in chrome; they have poor GPU/font portability.

## Interaction

Keyboard-first behavior is mandatory. Pointer affordances should match visible
regions exactly:

- title band empty space drags the window
- tab rail tabs switch and drag tabs
- tab close targets exist only when the rail is visible
- new-tab target remains in the title band in both single-tab and multi-tab
  states

Hover and pressed states should be stable in size. No hover state should shift
layout.

## Screenshots

`ulttyctl screenshot` is the canonical visual capture path for product review
and regression checks. It captures the renderer output, not the compositor
window frame.

Screenshot acceptance must include:

- single-tab capture shows title/control band and no lower tab rail
- multi-tab capture shows title/control band plus lower tab rail
- window controls are in the upper band
- tab labels and close targets are in the lower band
- terminal content begins below the active chrome height

Use compositor tools such as `gnome-screenshot`, `grim`, `import`, or
`xdotool` only for desktop/window-manager debugging. They are not the product
regression path.

## Implementation Notes

Keep visual constants named by role, not by incidental shape. Prefer
`TITLE_BAR_HEIGHT`, `TAB_ROW_HEIGHT`, and tab/control hit-testing helpers over
anonymous geometry in event handling.

Renderer, input, and screenshot paths must share the same state contract:
`TabBarState::height()` returns the active chrome height, and
`TabBarState::shows_tab_row()` decides whether tab rail interactions and pixels
exist.
