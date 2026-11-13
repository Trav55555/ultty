# Native rendering snapshots

`scene::view::NativeRenderSource` supplies read-only rendering state. Live
`NativeSession` owners and attachment `NativeReplica` models use the same
renderer. A replica owns a `SceneTree` and display data. It has no application
socket, native message dispatcher, output queue, or process ownership.

`NativeSnapshot::capture` runs on the owner thread between mutations. A newer
open frame leaves the previous committed scene available for capture; pending
frame operations remain at the owner. The snapshot includes region order and
parents, constraints, computed layout, styles, content, semantics, metadata,
scroll offsets, focus, stream markers, inline surface order, and placed anchor
tags. Images retain their inline RGBA data. GPU textures are recreated by the
renderer from this content rather than copied across the attachment.

Hydration validates tree identities and edges before building regions directly.
Only constraints, style, content, metadata, and semantics declarations are
decoded. Wrong-target, duplicate, incomplete, and executable session operations
are refused. Clipboard, input, subscriptions, frame commands, keepalives, and
transport configuration never reach native session handlers through this path.

Retained text needs a separate snapshot representation. Appends can grow a
region beyond the native codec's one-message string limit. Text, rich-text span
text and styles, and appendable Markdown therefore use bounded snapshot-only
payloads and an empty declaration skeleton. The native wire decoder keeps its
existing limits. The snapshot permits at most 4 MiB retained text per region
and the native span-count limit. Encoded span styles are decoded independently.

Each native snapshot has a 16 MiB storage budget. The enclosing complete
attachment snapshot also has a 16 MiB encoded writer limit across VT data,
native scenes, and all panes. Region, tree-depth, surface, finite geometry, and
text limits are checked before construction. Geometry is bounded to
plus or minus 2^20 cells. Unsupported or oversized scenes return an error rather
than publishing a partial replacement.

The terminal's displayed generation and optional committed app frame ID remain
distinct. Legacy native clients can have no committed semantic-action frame.
Replicas preserve these identities for observation; input authority and current
subscription routing remain at the host. Pointer hit testing shares the live
session's committed geometry helper. Replacing a replica marks its render and
scroll caches dirty; the viewer calls `mark_clean` after rebuilding them.
