//! Glyph atlas for caching rendered glyphs.
//!
//! Uses a texture atlas to store rasterized glyphs for efficient GPU rendering.
//!
//! When the atlas runs out of space (or the cache hits its cap) the eviction
//! is deferred to the next [`GlyphAtlas::begin_frame`]: quads already emitted
//! in the current frame keep referencing valid slots, the glyph that did not
//! fit is skipped once, and [`GlyphAtlas::frame_incomplete`] tells the caller
//! to schedule another frame.

use cosmic_text::{CacheKey, SwashCache, SwashContent, SwashImage};
use std::collections::HashMap;
use std::sync::Arc;

/// Size of the glyph atlas texture.
const ATLAS_SIZE: u32 = 2048;

/// Padding between glyphs in the atlas.
const GLYPH_PADDING: u32 = 1;

/// Pixel rows at the top of the atlas reserved for the solid white block
/// that decoration quads (underline, strikethrough) sample.
const RESERVED_ROWS: u32 = 2;

/// Maximum number of cached glyphs before triggering cleanup.
const MAX_CACHED_GLYPHS: usize = 4096;

/// Information about a cached glyph.
#[derive(Debug, Clone, Copy)]
pub struct GlyphInfo {
    /// UV coordinates in the atlas (x0, y0, x1, y1)
    pub uv: [f32; 4],
    /// Glyph size in pixels
    pub width: f32,
    pub height: f32,
    /// Offset from baseline
    pub left: f32,
    pub top: f32,
    /// Whether this glyph is a color emoji (true) or monochrome (false)
    pub is_color: bool,
}

/// Entry in the glyph cache with LRU tracking.
#[derive(Debug, Clone, Copy)]
struct CacheEntry {
    info: GlyphInfo,
    /// Last access time (monotonic counter)
    last_access: u64,
}

/// Row in the atlas for shelf-packing algorithm.
struct AtlasRow {
    y: u32,
    height: u32,
    x: u32,
}

/// A glyph slot handed out by [`AtlasLayout::place`].
#[derive(Debug, Clone, Copy)]
struct PlacedGlyph {
    /// Pixel origin of the slot in the atlas texture.
    x: u32,
    y: u32,
    info: GlyphInfo,
}

/// GPU-free bookkeeping for the atlas: shelf packing, the glyph cache and
/// the deferred-eviction flags.
///
/// Kept separate from the texture so the eviction policy can be unit-tested
/// without a device.
struct AtlasLayout {
    /// Cached glyphs with LRU tracking
    cache: HashMap<CacheKey, CacheEntry>,
    /// Current rows in the atlas (shelf packing)
    rows: Vec<AtlasRow>,
    /// Current y position for new rows
    current_y: u32,
    /// Access counter for LRU tracking
    access_counter: u64,
    /// Number of cache hits (for stats)
    cache_hits: u64,
    /// Number of cache misses (for stats)
    cache_misses: u64,
    /// The atlas ran out of space (or the cache hit its cap) this frame.
    /// Honoured at the next `begin_frame`, never mid-frame.
    needs_eviction: bool,
    /// At least one glyph could not be placed this frame.
    frame_incomplete: bool,
}

impl AtlasLayout {
    fn new() -> Self {
        Self {
            cache: HashMap::with_capacity(1024),
            rows: Vec::new(),
            current_y: RESERVED_ROWS,
            access_counter: 0,
            cache_hits: 0,
            cache_misses: 0,
            needs_eviction: false,
            frame_incomplete: false,
        }
    }

    /// Start a frame: apply any eviction requested by the previous frame.
    ///
    /// Nothing emitted in the previous frame is still in flight for the CPU
    /// side at this point, so resetting every slot is safe.
    fn begin_frame(&mut self) {
        if self.needs_eviction {
            self.reset();
        }
        self.frame_incomplete = false;
    }

    fn frame_incomplete(&self) -> bool {
        self.frame_incomplete
    }

    /// Look up a cached glyph, bumping its LRU stamp on a hit.
    fn get(&mut self, cache_key: CacheKey) -> Option<GlyphInfo> {
        match self.cache.get_mut(&cache_key) {
            Some(entry) => {
                self.access_counter += 1;
                entry.last_access = self.access_counter;
                self.cache_hits += 1;
                Some(entry.info)
            }
            None => {
                self.cache_misses += 1;
                None
            }
        }
    }

    /// Cache a glyph that has no pixels (spaces, zero-width marks).
    fn insert_empty(&mut self, cache_key: CacheKey, left: f32, top: f32) -> GlyphInfo {
        let info = GlyphInfo {
            uv: [0.0, 0.0, 0.0, 0.0],
            width: 0.0,
            height: 0.0,
            left,
            top,
            is_color: false,
        };
        self.insert(cache_key, info);
        info
    }

    /// Reserve a slot for a `width` x `height` glyph and cache it.
    ///
    /// Returns `None` when the atlas is full; the eviction is then deferred
    /// to the next `begin_frame` and the frame is marked incomplete.
    fn place(
        &mut self,
        cache_key: CacheKey,
        width: u32,
        height: u32,
        left: f32,
        top: f32,
        is_color: bool,
    ) -> Option<PlacedGlyph> {
        let Some((x, y)) = self.allocate(width, height) else {
            tracing::debug!(
                width,
                height,
                "Glyph atlas full; skipping glyph until eviction at next frame"
            );
            self.needs_eviction = true;
            self.frame_incomplete = true;
            return None;
        };

        let atlas_size = ATLAS_SIZE as f32;
        let info = GlyphInfo {
            uv: [
                x as f32 / atlas_size,
                y as f32 / atlas_size,
                (x + width) as f32 / atlas_size,
                (y + height) as f32 / atlas_size,
            ],
            width: width as f32,
            height: height as f32,
            left,
            top,
            is_color,
        };
        self.insert(cache_key, info);
        Some(PlacedGlyph { x, y, info })
    }

    fn insert(&mut self, cache_key: CacheKey, info: GlyphInfo) {
        self.access_counter += 1;
        self.cache.insert(
            cache_key,
            CacheEntry {
                info,
                last_access: self.access_counter,
            },
        );
        if self.cache.len() >= MAX_CACHED_GLYPHS {
            self.needs_eviction = true;
        }
    }

    /// Allocate space in the atlas using shelf packing.
    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let width = width + GLYPH_PADDING;
        let height = height + GLYPH_PADDING;

        // Try to fit in existing row
        for row in &mut self.rows {
            if row.height >= height && row.x + width <= ATLAS_SIZE {
                let x = row.x;
                row.x += width;
                return Some((x, row.y));
            }
        }

        // Need a new row
        if self.current_y + height > ATLAS_SIZE {
            return None;
        }

        let y = self.current_y;
        self.current_y += height;

        self.rows.push(AtlasRow {
            y,
            height,
            x: width,
        });

        Some((0, y))
    }

    /// Drop every cached glyph and slot.
    ///
    /// Shelf packing cannot free individual slots, and resetting allocation
    /// invalidates every UV, so the whole cache goes. Glyphs still needed are
    /// re-rasterized on their next access.
    fn reset(&mut self) {
        let evicted = self.cache.len();
        self.cache.clear();
        self.rows.clear();
        self.current_y = RESERVED_ROWS;
        self.needs_eviction = false;

        tracing::debug!("Evicted all {} glyphs from atlas", evicted);
    }

    fn stats(&self) -> AtlasStats {
        AtlasStats {
            cached_glyphs: self.cache.len(),
            atlas_usage: self.current_y - RESERVED_ROWS,
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
        }
    }
}

/// Glyph atlas for caching rendered glyphs.
pub struct GlyphAtlas {
    /// The texture atlas
    texture: wgpu::Texture,
    /// Texture view
    _view: wgpu::TextureView,
    /// Bind group for the atlas
    bind_group: wgpu::BindGroup,
    /// Bind group layout
    bind_group_layout: wgpu::BindGroupLayout,
    /// Slot allocation and glyph cache
    layout: AtlasLayout,
    /// Swash cache for rasterization
    swash_cache: SwashCache,
    /// Queue reference
    queue: Arc<wgpu::Queue>,
}

impl GlyphAtlas {
    /// Create a new glyph atlas.
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        // Create the atlas texture
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph_atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph_atlas_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph_atlas_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // Solid white block for decoration quads; it lives in the reserved
        // rows so no glyph slot ever overwrites it.
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8; (RESERVED_ROWS * RESERVED_ROWS * 4) as usize],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(RESERVED_ROWS * 4),
                rows_per_image: Some(RESERVED_ROWS),
            },
            wgpu::Extent3d {
                width: RESERVED_ROWS,
                height: RESERVED_ROWS,
                depth_or_array_layers: 1,
            },
        );

        Self {
            texture,
            _view: view,
            bind_group,
            bind_group_layout,
            layout: AtlasLayout::new(),
            swash_cache: SwashCache::new(),
            queue,
        }
    }

    /// Texture coordinate at the centre of the solid white block.
    ///
    /// A quad whose four corners all use this coordinate samples pure white
    /// with full alpha, so the text pipeline renders it in the vertex color.
    pub fn solid_uv(&self) -> [f32; 2] {
        let centre = RESERVED_ROWS as f32 * 0.5 / ATLAS_SIZE as f32;
        [centre, centre]
    }

    /// Get the bind group layout.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    /// Get the bind group.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Start a frame. Call before emitting any quad that samples the atlas;
    /// this is the only point at which slots are evicted.
    pub fn begin_frame(&mut self) {
        self.layout.begin_frame();
    }

    /// Whether a glyph was skipped this frame because the atlas was full.
    /// The caller should schedule another frame so it gets drawn.
    pub fn frame_incomplete(&self) -> bool {
        self.layout.frame_incomplete()
    }

    /// Get or cache a glyph.
    ///
    /// Returns `None` for glyphs that cannot be rasterized and for glyphs
    /// that do not fit this frame (see [`Self::frame_incomplete`]).
    pub fn get_glyph(
        &mut self,
        font_system: &mut cosmic_text::FontSystem,
        cache_key: CacheKey,
    ) -> Option<GlyphInfo> {
        if let Some(info) = self.layout.get(cache_key) {
            return Some(info);
        }

        // Rasterize the glyph using uncached method to get owned data
        let image = self
            .swash_cache
            .get_image_uncached(font_system, cache_key)?;

        // Process the image
        self.process_image(cache_key, image)
    }

    /// Process a rasterized glyph image.
    fn process_image(&mut self, cache_key: CacheKey, image: SwashImage) -> Option<GlyphInfo> {
        // Skip empty glyphs
        if image.placement.width == 0 || image.placement.height == 0 {
            return Some(self.layout.insert_empty(
                cache_key,
                image.placement.left as f32,
                image.placement.top as f32,
            ));
        }

        // Convert image data based on content type
        let (data, is_color) = match image.content {
            SwashContent::Mask => {
                // Expand 1-byte alpha to RGBA: (255, 255, 255, alpha)
                let rgba: Vec<u8> = image
                    .data
                    .iter()
                    .flat_map(|&a| [255, 255, 255, a])
                    .collect();
                (rgba, false)
            }
            SwashContent::Color => {
                // Keep full RGBA color data (emoji)
                (image.data.clone(), true)
            }
            SwashContent::SubpixelMask => {
                // Treat as mono, use first subpixel channel as alpha
                let rgba: Vec<u8> = image
                    .data
                    .chunks(3)
                    .flat_map(|c| [255, 255, 255, c[0]])
                    .collect();
                (rgba, false)
            }
        };

        // Find space in the atlas
        let placed = self.layout.place(
            cache_key,
            image.placement.width,
            image.placement.height,
            image.placement.left as f32,
            image.placement.top as f32,
            is_color,
        )?;

        // Upload to texture (4 bytes per pixel for Rgba8Unorm)
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: placed.x,
                    y: placed.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(image.placement.width * 4),
                rows_per_image: Some(image.placement.height),
            },
            wgpu::Extent3d {
                width: image.placement.width,
                height: image.placement.height,
                depth_or_array_layers: 1,
            },
        );

        Some(placed.info)
    }

    /// Clear the atlas cache.
    pub fn clear(&mut self) {
        self.layout.reset();
    }

    /// Get cache statistics.
    pub fn stats(&self) -> AtlasStats {
        self.layout.stats()
    }
}

/// Statistics about the glyph atlas.
#[derive(Debug, Clone, Copy)]
pub struct AtlasStats {
    /// Number of cached glyphs
    pub cached_glyphs: usize,
    /// Current Y position in atlas (indicates usage)
    pub atlas_usage: u32,
    /// Number of cache hits
    pub cache_hits: u64,
    /// Number of cache misses
    pub cache_misses: u64,
}

impl AtlasStats {
    /// Calculate cache hit rate as a percentage.
    pub fn hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            (self.cache_hits as f64 / total as f64) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_text::CacheKeyFlags;

    /// Glyph edge (px) chosen so a whole number of glyphs fit per shelf:
    /// (128 + padding) * 15 = 1935 <= 2048, and 15 shelves fit below the
    /// reserved rows.
    const GLYPH_PX: u32 = 128;
    const GLYPHS_PER_SHELF: u32 = ATLAS_SIZE / (GLYPH_PX + GLYPH_PADDING);
    const SHELVES: u32 = (ATLAS_SIZE - RESERVED_ROWS) / (GLYPH_PX + GLYPH_PADDING);
    const CAPACITY: u16 = (GLYPHS_PER_SHELF * SHELVES) as u16;

    fn key(glyph_id: u16) -> CacheKey {
        CacheKey::new(
            cosmic_text::fontdb::ID::dummy(),
            glyph_id,
            14.0,
            (0.0, 0.0),
            CacheKeyFlags::empty(),
        )
        .0
    }

    fn place(layout: &mut AtlasLayout, glyph_id: u16) -> Option<PlacedGlyph> {
        layout.place(key(glyph_id), GLYPH_PX, GLYPH_PX, 0.0, 0.0, false)
    }

    #[test]
    fn full_atlas_defers_eviction_to_next_frame_and_then_fits_missing_glyphs() {
        let mut layout = AtlasLayout::new();
        layout.begin_frame();

        let first = place(&mut layout, 0).expect("empty atlas has room");
        let total = CAPACITY + 40;
        let missing: Vec<u16> = (1..total)
            .filter(|&id| place(&mut layout, id).is_none())
            .collect();

        assert_eq!(missing.len(), 40, "everything past capacity is skipped");
        assert!(layout.frame_incomplete());
        // Nothing emitted earlier in this frame was disturbed.
        assert_eq!(layout.stats().cached_glyphs, usize::from(CAPACITY));
        assert_eq!(layout.get(key(0)).unwrap().uv, first.info.uv);

        // The next frame starts from an empty atlas and every glyph that was
        // skipped can now be placed.
        layout.begin_frame();
        assert!(!layout.frame_incomplete());
        assert_eq!(layout.stats().cached_glyphs, 0);
        for id in &missing {
            assert!(
                place(&mut layout, *id).is_some(),
                "glyph {id} still missing"
            );
        }
        assert!(!layout.frame_incomplete());
        assert_eq!(layout.stats().cached_glyphs, missing.len());
    }

    #[test]
    fn begin_frame_keeps_cache_when_nothing_overflowed() {
        let mut layout = AtlasLayout::new();
        layout.begin_frame();
        let placed = place(&mut layout, 7).unwrap();

        layout.begin_frame();

        assert_eq!(layout.get(key(7)).unwrap().uv, placed.info.uv);
        assert_eq!(layout.stats().cached_glyphs, 1);
    }

    #[test]
    fn cache_cap_is_enforced_at_next_frame_not_mid_frame() {
        let mut layout = AtlasLayout::new();
        layout.begin_frame();
        for id in 0..MAX_CACHED_GLYPHS {
            layout.insert_empty(key(id as u16), 0.0, 0.0);
        }
        assert_eq!(layout.stats().cached_glyphs, MAX_CACHED_GLYPHS);
        assert!(!layout.frame_incomplete(), "cap alone does not skip glyphs");

        layout.begin_frame();

        assert_eq!(layout.stats().cached_glyphs, 0);
    }

    #[test]
    fn glyph_slots_never_overlap_the_reserved_solid_block() {
        let mut layout = AtlasLayout::new();
        let placed = layout.place(key(1), 4, 4, 0.0, 0.0, false).unwrap();

        assert_eq!(placed.x, 0);
        assert_eq!(placed.y, RESERVED_ROWS);
        assert_eq!(layout.stats().atlas_usage, 4 + GLYPH_PADDING);
    }

    #[test]
    fn glyph_info_layout() {
        let info = GlyphInfo {
            uv: [0.1, 0.2, 0.3, 0.4],
            width: 10.0,
            height: 20.0,
            left: 1.0,
            top: 15.0,
            is_color: false,
        };

        assert_eq!(info.uv[0], 0.1);
        assert_eq!(info.width, 10.0);
        assert_eq!(info.top, 15.0);
    }
}
