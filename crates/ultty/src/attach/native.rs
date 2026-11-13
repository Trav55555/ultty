//! Bounded committed native scenes for read-only attachment rendering.
//!
//! Declarative fields reuse the native codec, but hydration constructs regions
//! directly. No application message is dispatched through a live session.

use std::collections::{HashMap, HashSet, VecDeque};

use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use ultty_protocol::{
    AppMessage, Content, Decode, Encode, RegionId, Span, Style, WidgetContent, MAX_REGION_COUNT,
    MAX_SPAN_COUNT, MAX_SURFACES, MAX_SURFACE_ROWS, MIN_SURFACE_ROWS,
};

use crate::protocol::session::PlacedSurface;
use crate::protocol::NativeSession;
use crate::scene::region::{ComputedLayout, Region};
use crate::scene::tree::MAX_TREE_DEPTH;
use crate::scene::view::NativeRenderSource;
use crate::scene::SceneTree;

/// Native snapshot schema version.
pub const NATIVE_SNAPSHOT_VERSION: u16 = 1;
/// Maximum retained native snapshot payload, also bounded by the outer writer.
pub const MAX_NATIVE_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// One committed native session, without transport or pending frame state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeSnapshot {
    /// Native snapshot schema.
    pub version: u16,
    /// Owner-scoped native session identity.
    pub session_id: u64,
    /// Terminal-owned displayed scene generation.
    pub generation: u64,
    /// Last committed app frame, absent for legacy or invalidated action frames.
    pub frame_id: Option<u32>,
    /// Canonical viewport in cells.
    pub viewport: [f32; 2],
    /// Whether the scene replaces its pane's VT grid.
    pub full_scene: bool,
    /// Focused region, if any.
    pub focused_region: Option<u32>,
    /// Every region, including full-scene and surface roots.
    pub regions: Vec<NativeRegionSnapshot>,
    /// Surface roots in creation order.
    pub surfaces: Vec<NativeSurfaceSnapshot>,
    /// Placed surfaces in drawing order, with VT anchor tags.
    pub placed_surfaces: Vec<NativePlacedSurfaceSnapshot>,
}

/// One region's bounded declaration and explicit retained geometry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeRegionSnapshot {
    /// Region identity.
    pub id: u32,
    /// Parent, absent only for full-scene and surface roots.
    pub parent: Option<u32>,
    /// Children in drawing order.
    pub children: Vec<u32>,
    /// x, y, width, height, content x, content y, content width, content height.
    pub computed: [f32; 8],
    /// Explicit scroll offset in cells.
    pub scroll: [f32; 2],
    /// Whether appended content participates in derived log semantics.
    pub appended: bool,
    /// Whether a content stream remains open.
    pub streaming: bool,
    /// Exactly one constraints, style, content, metadata, and semantics message
    /// for this ID. All other native operations are refused during hydration.
    pub declaration: Vec<u8>,
    /// Appendable retained text can exceed one native message's string limit.
    /// Its bounded snapshot payload restores the declaration's empty skeleton.
    pub text: Option<NativeTextSnapshot>,
}

/// Snapshot-only retained text, preserving native message size limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NativeTextSnapshot {
    /// Plain text, whose formatting remains in its declaration skeleton.
    Plain(String),
    /// Styled spans, whose text and encoded styles are restored together.
    Rich(Vec<NativeSpanSnapshot>),
    /// Legacy appendable markdown source.
    Markdown(String),
}

/// A retained rich-text span.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeSpanSnapshot {
    /// Original UTF-8 text.
    pub text: String,
    /// Encoded style, bounded independently and decoded without session handlers.
    pub style: Vec<u8>,
}

/// A retained inline surface root.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NativeSurfaceSnapshot {
    /// Surface root identity.
    pub id: u32,
    /// Reserved rows in cells.
    pub rows: u16,
}

/// An inline surface's read-only association with a VT anchor.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NativePlacedSurfaceSnapshot {
    /// Surface root identity.
    pub id: u32,
    /// Reserved rows in cells.
    pub rows: u16,
    /// VT anchor tag whose current line determines screen placement.
    pub tag: u16,
}

/// A hydrated rendering model. It has no socket, output queue or command API.
pub struct NativeReplica {
    session_id: u64,
    generation: u64,
    frame_id: Option<u32>,
    viewport: (f32, f32),
    scene: SceneTree,
    scroll: HashMap<RegionId, (f32, f32)>,
    focused_region: Option<RegionId>,
    placed_surfaces: Vec<PlacedSurface>,
    render_dirty: bool,
    scroll_dirty: bool,
}

impl NativeReplica {
    /// Route a pointer using the same committed geometry as a live session.
    pub fn hit_test(&self, x: f32, y: f32) -> (RegionId, f32, f32) {
        crate::scene::view::hit_test(self, x, y)
    }
    /// Owner-scoped identity of this model.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }
    /// Displayed generation represented by the complete replacement.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Optional committed application frame represented by this replacement.
    pub fn committed_frame_id(&self) -> Option<u32> {
        self.frame_id
    }
    /// Focused region represented by this replacement.
    pub fn focused_region(&self) -> Option<RegionId> {
        self.focused_region
    }
    /// Acknowledge that renderer caches now represent this immutable model.
    pub fn mark_clean(&mut self) {
        self.render_dirty = false;
        self.scroll_dirty = false;
    }
}

impl NativeRenderSource for NativeReplica {
    fn session_id(&self) -> u64 {
        self.session_id
    }
    fn revision(&self) -> u64 {
        self.generation
    }
    fn scene(&self) -> &SceneTree {
        &self.scene
    }
    fn viewport(&self) -> (f32, f32) {
        self.viewport
    }
    fn is_full_scene(&self) -> bool {
        self.scene
            .get(RegionId::ROOT)
            .is_some_and(|root| !root.children.is_empty())
    }
    fn scroll_offset(&self, id: RegionId) -> (f32, f32) {
        self.scroll.get(&id).copied().unwrap_or_default()
    }
    fn is_streaming(&self, id: RegionId) -> bool {
        self.scene.get(id).is_some_and(|region| region.streaming)
    }
    fn is_render_dirty(&self) -> bool {
        self.render_dirty
    }
    fn is_scroll_dirty(&self) -> bool {
        self.scroll_dirty
    }
    fn placed_surfaces(&self, visit: &mut dyn FnMut(PlacedSurface)) {
        for surface in &self.placed_surfaces {
            visit(*surface);
        }
    }
}

impl NativeSnapshot {
    /// Capture the last committed scene, including while newer frame operations
    /// remain buffered. Call on the session's owner thread between mutations.
    pub fn capture(session: &NativeSession) -> Result<Self, String> {
        let mut regions = Vec::new();
        let mut budget = 0usize;
        for region in session.scene().iter_all() {
            let mut declaration = BytesMut::new();
            let mut content = region.content.clone();
            let text = extract_text(&mut content);
            if let Some(text) = &text {
                charge(&mut budget, text_budget(text)?)?;
            }
            for message in [
                AppMessage::SetConstraints {
                    id: region.id,
                    constraints: region.constraints.clone(),
                },
                AppMessage::SetStyle {
                    id: region.id,
                    style: region.style.clone(),
                },
                AppMessage::SetContent {
                    id: region.id,
                    content,
                },
                AppMessage::SetRegionMetadata {
                    id: region.id,
                    metadata: region.metadata.clone(),
                },
                AppMessage::SetSemantics {
                    id: region.id,
                    semantics: region.semantics().clone(),
                },
            ] {
                message.encode(&mut declaration);
                if declaration.len() > MAX_NATIVE_SNAPSHOT_BYTES {
                    return Err("native snapshot declaration exceeds byte limit".into());
                }
            }
            charge(&mut budget, declaration.len())?;
            charge(
                &mut budget,
                80usize.saturating_add(region.children.len().saturating_mul(4)),
            )?;
            let layout = region.computed;
            let scroll = session.scroll_offset(region.id);
            regions.push(NativeRegionSnapshot {
                id: region.id.0,
                parent: region.parent.map(|id| id.0),
                children: region.children.iter().map(|id| id.0).collect(),
                computed: [
                    layout.x,
                    layout.y,
                    layout.width,
                    layout.height,
                    layout.content_x,
                    layout.content_y,
                    layout.content_width,
                    layout.content_height,
                ],
                scroll: [scroll.0, scroll.1],
                appended: region.appended,
                streaming: session.is_streaming(region.id),
                declaration: declaration.to_vec(),
                text,
            });
        }
        let viewport = session.viewport();
        let snapshot = Self {
            version: NATIVE_SNAPSHOT_VERSION,
            session_id: session.session_id(),
            generation: session.shown_generation(),
            frame_id: session.committed_frame_id().map(|id| id.0),
            viewport: [viewport.0, viewport.1],
            full_scene: session.is_full_scene(),
            focused_region: session.focused_region().map(|id| id.0),
            regions,
            surfaces: session
                .scene()
                .surfaces()
                .iter()
                .map(|surface| NativeSurfaceSnapshot {
                    id: surface.id.0,
                    rows: surface.rows,
                })
                .collect(),
            placed_surfaces: session
                .placed_surfaces()
                .map(|surface| NativePlacedSurfaceSnapshot {
                    id: surface.id.0,
                    rows: surface.rows,
                    tag: surface.tag,
                })
                .collect(),
        };
        snapshot.validate_structure()?;
        Ok(snapshot)
    }

    /// Validate and hydrate an independent scene without native session handlers.
    pub fn to_view(&self) -> Result<NativeReplica, String> {
        let order = self.validate_structure()?;
        let mut decoded: HashMap<_, _> = self
            .regions
            .iter()
            .map(|region| decode_region(region).map(|decoded| (region.id, decoded)))
            .collect::<Result<_, _>>()?;
        let surface_rows: HashMap<_, _> = self
            .surfaces
            .iter()
            .map(|surface| (surface.id, surface.rows))
            .collect();
        let mut scene = SceneTree::new();
        for id in order {
            let region = decoded.remove(&id).ok_or("missing decoded native region")?;
            if id == RegionId::ROOT.0 {
                *scene
                    .get_mut(RegionId::ROOT)
                    .ok_or("missing replica root")? = region;
            } else if let Some(rows) = surface_rows.get(&id) {
                scene
                    .try_insert_surface(region, *rows)
                    .map_err(|error| error.to_string())?;
            } else {
                scene
                    .try_insert_at(region, usize::MAX)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(NativeReplica {
            session_id: self.session_id,
            generation: self.generation,
            frame_id: self.frame_id,
            viewport: (self.viewport[0], self.viewport[1]),
            scene,
            scroll: self
                .regions
                .iter()
                .map(|region| (RegionId(region.id), (region.scroll[0], region.scroll[1])))
                .collect(),
            focused_region: self.focused_region.map(RegionId),
            placed_surfaces: self
                .placed_surfaces
                .iter()
                .map(|surface| PlacedSurface {
                    id: RegionId(surface.id),
                    rows: surface.rows,
                    tag: surface.tag,
                })
                .collect(),
            render_dirty: true,
            scroll_dirty: true,
        })
    }

    /// Validate bounded identities, roots, layout and ordered edges. Returns a
    /// parent-before-child traversal for direct region construction.
    fn validate_structure(&self) -> Result<Vec<u32>, String> {
        if self.version != NATIVE_SNAPSHOT_VERSION
            || self.regions.is_empty()
            || self.regions.len() > MAX_REGION_COUNT
            || self.surfaces.len() > MAX_SURFACES
            || self.placed_surfaces.len() > self.surfaces.len()
            || !self
                .viewport
                .iter()
                .all(|value| finite(*value) && *value >= 0.0)
            || self.frame_id == Some(0)
        {
            return Err("invalid native snapshot header".into());
        }
        let mut budget = 0;
        let mut regions = HashMap::new();
        let mut edge_count = 0usize;
        for region in &self.regions {
            charge(&mut budget, region.declaration.len())?;
            if let Some(text) = &region.text {
                charge(&mut budget, text_budget(text)?)?;
            }
            charge(
                &mut budget,
                80usize.saturating_add(region.children.len().saturating_mul(4)),
            )?;
            edge_count = edge_count.saturating_add(region.children.len());
            if regions.insert(region.id, region).is_some()
                || region.children.len() > MAX_REGION_COUNT
                || edge_count >= self.regions.len()
                || !region.computed.iter().all(|value| finite(*value))
                || ![2, 3, 6, 7]
                    .iter()
                    .all(|index| region.computed[*index] >= 0.0)
                || !region.scroll.iter().all(|value| finite(*value))
            {
                return Err("invalid native region structure or geometry".into());
            }
        }
        let mut roots = vec![RegionId::ROOT.0];
        let mut surfaces = HashMap::new();
        for surface in &self.surfaces {
            if surface.id == RegionId::ROOT.0
                || !(MIN_SURFACE_ROWS..=MAX_SURFACE_ROWS).contains(&surface.rows)
                || surfaces.insert(surface.id, surface.rows).is_some()
            {
                return Err("invalid native surface root".into());
            }
            roots.push(surface.id);
        }
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        for root in roots {
            if regions
                .get(&root)
                .ok_or("missing native root")?
                .parent
                .is_some()
            {
                return Err("native root has a parent".into());
            }
            seen.insert(root);
            queue.push_back((root, 0usize));
        }
        let mut order = Vec::with_capacity(self.regions.len());
        while let Some((id, depth)) = queue.pop_front() {
            if depth > MAX_TREE_DEPTH {
                return Err("native tree exceeds depth limit".into());
            }
            order.push(id);
            for child in &regions[&id].children {
                if regions.get(child).ok_or("missing native child")?.parent != Some(id)
                    || !seen.insert(*child)
                {
                    return Err("native tree has a cycle, duplicate edge or wrong parent".into());
                }
                queue.push_back((*child, depth + 1));
            }
        }
        if order.len() != regions.len()
            || self
                .focused_region
                .is_some_and(|id| !regions.contains_key(&id))
            || self.full_scene == regions[&0].children.is_empty()
        {
            return Err("native snapshot contains unreachable regions or invalid focus".into());
        }
        let mut placed = HashSet::new();
        let mut tags = HashSet::new();
        for surface in &self.placed_surfaces {
            if surfaces.get(&surface.id) != Some(&surface.rows)
                || !placed.insert(surface.id)
                || !tags.insert(surface.tag)
            {
                return Err("invalid native surface placement".into());
            }
        }
        Ok(order)
    }
}

fn finite(value: f32) -> bool {
    value.is_finite() && value.abs() <= 1_048_576.0
}

fn charge(budget: &mut usize, bytes: usize) -> Result<(), String> {
    *budget = budget
        .checked_add(bytes)
        .filter(|total| *total <= MAX_NATIVE_SNAPSHOT_BYTES)
        .ok_or("native snapshot exceeds byte limit")?;
    Ok(())
}

fn extract_text(content: &mut Content) -> Option<NativeTextSnapshot> {
    match content {
        Content::Text(text) => Some(NativeTextSnapshot::Plain(std::mem::take(&mut text.text))),
        Content::RichText(rich) => Some(NativeTextSnapshot::Rich(
            std::mem::take(&mut rich.spans)
                .into_iter()
                .map(|span| NativeSpanSnapshot {
                    text: span.text,
                    style: span.style.encode_to_bytes().to_vec(),
                })
                .collect(),
        )),
        Content::Widget(WidgetContent::Markdown(markdown)) => Some(NativeTextSnapshot::Markdown(
            std::mem::take(&mut markdown.source),
        )),
        _ => None,
    }
}

fn text_budget(text: &NativeTextSnapshot) -> Result<usize, String> {
    let (text_bytes, style_bytes) = match text {
        NativeTextSnapshot::Plain(text) | NativeTextSnapshot::Markdown(text) => (text.len(), 0),
        NativeTextSnapshot::Rich(spans) => {
            if spans.len() > MAX_SPAN_COUNT {
                return Err("native snapshot has too many retained spans".into());
            }
            let mut text_bytes = 0usize;
            let mut style_bytes = 0usize;
            for span in spans {
                if span.style.len() > 1024 {
                    return Err("native span style exceeds limit".into());
                }
                text_bytes = text_bytes.saturating_add(span.text.len());
                style_bytes = style_bytes
                    .saturating_add(span.style.len())
                    .saturating_add(16);
            }
            (text_bytes, style_bytes)
        }
    };
    if text_bytes > 4 * 1024 * 1024 {
        return Err("native retained text exceeds region limit".into());
    }
    text_bytes
        .checked_add(style_bytes)
        .ok_or_else(|| "native text size overflow".into())
}

fn restore_text(content: &mut Content, snapshot: &NativeTextSnapshot) -> Result<(), String> {
    match (content, snapshot) {
        (Content::Text(text), NativeTextSnapshot::Plain(value)) if text.text.is_empty() => {
            text.text = value.clone();
        }
        (Content::RichText(rich), NativeTextSnapshot::Rich(spans)) if rich.spans.is_empty() => {
            rich.spans = spans
                .iter()
                .map(|span| {
                    let mut input = &span.style[..];
                    let style = Style::decode(&mut input)
                        .map_err(|_| "invalid native span style".to_string())?;
                    if !input.is_empty() {
                        return Err("trailing native span style bytes".into());
                    }
                    Ok(Span {
                        text: span.text.clone(),
                        style,
                    })
                })
                .collect::<Result<_, String>>()?;
        }
        (
            Content::Widget(WidgetContent::Markdown(markdown)),
            NativeTextSnapshot::Markdown(source),
        ) if markdown.source.is_empty() => {
            markdown.source = source.clone();
        }
        _ => return Err("native text payload does not match declaration skeleton".into()),
    }
    Ok(())
}

fn decode_region(snapshot: &NativeRegionSnapshot) -> Result<Region, String> {
    let id = RegionId(snapshot.id);
    let mut region = Region::new(id);
    region.parent = snapshot.parent.map(RegionId);
    let mut input = &snapshot.declaration[..];
    let mut fields = 0u8;
    while !input.is_empty() {
        let message = AppMessage::decode(&mut input)
            .map_err(|_| "malformed native region declaration".to_string())?;
        let bit = match message {
            AppMessage::SetConstraints {
                id: target,
                constraints,
            } if target == id => {
                region.constraints = constraints;
                1
            }
            AppMessage::SetStyle { id: target, style } if target == id => {
                region.style = style;
                2
            }
            AppMessage::SetContent {
                id: target,
                content,
            } if target == id => {
                region.content = content;
                4
            }
            AppMessage::SetRegionMetadata {
                id: target,
                metadata,
            } if target == id => {
                region.metadata = metadata;
                8
            }
            AppMessage::SetSemantics {
                id: target,
                semantics,
            } if target == id => {
                region.set_semantics(semantics);
                16
            }
            _ => {
                return Err(
                    "native replica refuses nondeclarative or wrong-target operation".into(),
                )
            }
        };
        if fields & bit != 0 {
            return Err("duplicate native declaration field".into());
        }
        fields |= bit;
    }
    if fields != 31 {
        return Err("incomplete native region declaration".into());
    }
    let layout = snapshot.computed;
    region.computed = ComputedLayout {
        x: layout[0],
        y: layout[1],
        width: layout[2],
        height: layout[3],
        content_x: layout[4],
        content_y: layout[5],
        content_width: layout[6],
        content_height: layout[7],
    };
    if let Some(text) = &snapshot.text {
        restore_text(&mut region.content, text)?;
    }
    region.appended = snapshot.appended;
    region.streaming = snapshot.streaming;
    Ok(region)
}

#[cfg(test)]
mod tests {
    use ultty_protocol::{
        Color, Constraints, Content, Dimension, FrameId, ImageScale, ImageWidget, RichText, Role,
        Semantics, Style, TextContent, WidgetContent, CAP_ALL_KNOWN, PROTOCOL_VERSION,
    };

    use super::*;
    use crate::terminal_manager::TabId;

    fn apply(session: &mut NativeSession, message: AppMessage) {
        assert!(session.handle_message_for_test(message).unwrap());
    }

    fn fixture() -> NativeSession {
        let mut session = NativeSession::new_in_band(7, TabId::new(0), None, Vec::new());
        session.set_viewport(80.0, 24.0);
        session.set_inline_surfaces(true);
        apply(
            &mut session,
            AppMessage::ClientHello {
                version: PROTOCOL_VERSION,
                requested_capabilities: CAP_ALL_KNOWN,
            },
        );
        apply(
            &mut session,
            AppMessage::BeginFrame {
                frame_id: FrameId(1),
            },
        );
        apply(
            &mut session,
            AppMessage::CreateRegion {
                id: RegionId(1),
                parent: RegionId::ROOT,
                insert_index: -1,
                constraints: Constraints {
                    width: Dimension::Cells(20.0),
                    height: Dimension::Cells(4.0),
                    ..Default::default()
                },
                style: Style::new().bg(Color::rgb(20, 30, 40)),
            },
        );
        apply(
            &mut session,
            AppMessage::SetContent {
                id: RegionId(1),
                content: Content::RichText(RichText::new().bold("before").text(" retained")),
            },
        );
        apply(
            &mut session,
            AppMessage::SetSemantics {
                id: RegionId(1),
                semantics: Semantics {
                    role: Role::Button,
                    label: Some("Run".into()),
                    ..Default::default()
                },
            },
        );
        apply(
            &mut session,
            AppMessage::SetRegionMetadata {
                id: RegionId(1),
                metadata: vec![("id".into(), "button-key".into())],
            },
        );
        apply(
            &mut session,
            AppMessage::StreamStart {
                id: RegionId(1),
                stream_id: 3,
            },
        );
        apply(
            &mut session,
            AppMessage::CreateRegion {
                id: RegionId(2),
                parent: RegionId(1),
                insert_index: -1,
                constraints: Constraints::default(),
                style: Style::default(),
            },
        );
        apply(
            &mut session,
            AppMessage::SetContent {
                id: RegionId(2),
                content: Content::Widget(WidgetContent::Image(ImageWidget {
                    data: vec![255, 10, 20, 255],
                    width: 1,
                    height: 1,
                    scale: ImageScale::Fit,
                })),
            },
        );
        apply(
            &mut session,
            AppMessage::CreateSurface {
                id: RegionId(9),
                rows: 2,
                constraints: Constraints::default(),
                style: Style::default(),
            },
        );
        apply(
            &mut session,
            AppMessage::SetContent {
                id: RegionId(9),
                content: Content::Text(TextContent::new("inline")),
            },
        );
        apply(
            &mut session,
            AppMessage::EndFrame {
                frame_id: FrameId(1),
            },
        );
        apply(&mut session, AppMessage::Focus { id: RegionId(1) });
        session.place_surface(RegionId(9), 17, 1).unwrap();
        session
    }

    #[test]
    fn test_native_snapshot_preserves_committed_scene_resources_and_placement() {
        let mut live = fixture();
        let snapshot = NativeSnapshot::capture(&live).unwrap();
        let wire = serde_json::to_vec(&snapshot).unwrap();
        let snapshot: NativeSnapshot = serde_json::from_slice(&wire).unwrap();
        let mut replica = snapshot.to_view().unwrap();
        assert_eq!(replica.session_id(), live.session_id());
        assert_eq!(replica.generation(), live.shown_generation());
        assert_eq!(replica.committed_frame_id(), Some(1));
        assert_eq!(replica.focused_region(), live.focused_region());
        assert_eq!(replica.viewport(), live.viewport());
        assert_eq!(replica.is_full_scene(), live.is_full_scene());
        for (x, y) in [(0.0, 0.0), (1.0, 1.0), (19.0, 3.0), (40.0, 20.0)] {
            assert_eq!(replica.hit_test(x, y), live.hit_test(x, y));
        }
        for region in live.scene().iter_all() {
            let copied = replica.scene().get(region.id).unwrap();
            assert_eq!(copied.parent, region.parent);
            assert_eq!(copied.children, region.children);
            assert_eq!(copied.content, region.content);
            assert_eq!(copied.constraints, region.constraints);
            assert_eq!(copied.style, region.style);
            assert_eq!(copied.metadata, region.metadata);
            assert_eq!(copied.semantics(), region.semantics());
            assert_eq!(copied.computed.x, region.computed.x);
            assert_eq!(
                copied.computed.content_height,
                region.computed.content_height
            );
            assert_eq!(
                replica.scroll_offset(region.id),
                live.scroll_offset(region.id)
            );
            assert_eq!(
                replica.is_streaming(region.id),
                live.is_streaming(region.id)
            );
        }
        let mut placed = Vec::new();
        replica.placed_surfaces(&mut |surface| placed.push(surface));
        assert_eq!(placed, live.placed_surfaces().collect::<Vec<_>>());
        assert!(replica.is_render_dirty());
        replica.mark_clean();
        assert!(!replica.is_render_dirty());
        assert!(!replica.is_scroll_dirty());
        // Newer buffered operations never leak into an attachment snapshot.
        apply(
            &mut live,
            AppMessage::BeginFrame {
                frame_id: FrameId(2),
            },
        );
        apply(
            &mut live,
            AppMessage::SetContent {
                id: RegionId(1),
                content: Content::Text(TextContent::new("uncommitted")),
            },
        );
        let during = NativeSnapshot::capture(&live).unwrap().to_view().unwrap();
        assert_eq!(
            during.scene().get(RegionId(1)).unwrap().content,
            replica.scene().get(RegionId(1)).unwrap().content
        );
        assert_eq!(during.committed_frame_id(), Some(1));
    }

    #[test]
    fn test_native_snapshot_rejects_operations_structure_and_resource_overflow() {
        let base = NativeSnapshot::capture(&fixture()).unwrap();
        let mut bad = base.clone();
        bad.regions[0].declaration = AppMessage::ClipboardSet {
            text: "must never execute".into(),
        }
        .encode_to_bytes()
        .to_vec();
        assert!(bad.to_view().is_err());
        let mut bad = base.clone();
        bad.regions[0].children.push(0);
        assert!(bad.to_view().is_err());
        let mut bad = base.clone();
        bad.regions[1].parent = Some(999);
        assert!(bad.to_view().is_err());
        let mut bad = base.clone();
        bad.regions[1].computed[0] = f32::NAN;
        assert!(bad.to_view().is_err());
        let mut bad = base.clone();
        bad.placed_surfaces[0].rows += 1;
        assert!(bad.to_view().is_err());
        let mut bad = base.clone();
        bad.regions[0].declaration = vec![0; MAX_NATIVE_SNAPSHOT_BYTES];
        assert!(bad.to_view().is_err());
        let mut bad = base;
        bad.regions[0].declaration.extend_from_within(..);
        assert!(bad.to_view().is_err());
    }

    #[test]
    fn test_native_snapshot_preserves_appended_text_beyond_single_message_caps() {
        let mut live = fixture();
        apply(
            &mut live,
            AppMessage::SetContent {
                id: RegionId(1),
                content: Content::Text(TextContent::new("x".repeat(1024 * 1024))),
            },
        );
        apply(
            &mut live,
            AppMessage::AppendContent {
                id: RegionId(1),
                text: "y".repeat(1024 * 1024),
            },
        );
        let snapshot = NativeSnapshot::capture(&live).unwrap();
        let view = snapshot.to_view().unwrap();
        assert_eq!(
            view.scene().get(RegionId(1)).unwrap().content,
            live.scene().get(RegionId(1)).unwrap().content
        );
        let spans = (0..MAX_SPAN_COUNT)
            .map(|_| Span {
                text: "x".into(),
                style: Style::new().bold(),
            })
            .collect();
        apply(
            &mut live,
            AppMessage::SetContent {
                id: RegionId(1),
                content: Content::RichText(RichText { spans }),
            },
        );
        for _ in 0..2 {
            apply(
                &mut live,
                AppMessage::AppendContent {
                    id: RegionId(1),
                    text: "y".repeat(1024 * 1024),
                },
            );
        }
        let view = NativeSnapshot::capture(&live).unwrap().to_view().unwrap();
        assert_eq!(
            view.scene().get(RegionId(1)).unwrap().content,
            live.scene().get(RegionId(1)).unwrap().content
        );
        if let Content::RichText(rich) = &view.scene().get(RegionId(1)).unwrap().content {
            assert_eq!(rich.spans.len(), MAX_SPAN_COUNT);
        } else {
            panic!("rich text was not preserved");
        }
        let mut bad = NativeSnapshot::capture(&live).unwrap();
        let rich = bad
            .regions
            .iter_mut()
            .find(|region| region.id == 1)
            .unwrap();
        if let Some(NativeTextSnapshot::Rich(spans)) = &mut rich.text {
            spans[0].style.push(0);
        }
        assert!(bad.to_view().is_err());
    }
}
