//! Glyph atlas for caching rendered glyphs.
//!
//! Uses a texture atlas to store rasterized glyphs for efficient GPU rendering.
//! Implements LRU eviction when the atlas becomes full.

use cosmic_text::{CacheKey, SwashCache, SwashContent, SwashImage};
use std::collections::HashMap;
use std::sync::Arc;

/// Size of the glyph atlas texture.
const ATLAS_SIZE: u32 = 2048;

/// Padding between glyphs in the atlas.
const GLYPH_PADDING: u32 = 1;

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
    /// Cached glyphs with LRU tracking
    cache: HashMap<CacheKey, CacheEntry>,
    /// Swash cache for rasterization
    swash_cache: SwashCache,
    /// Current rows in the atlas (shelf packing)
    rows: Vec<AtlasRow>,
    /// Current y position for new rows
    current_y: u32,
    /// Queue reference
    queue: Arc<wgpu::Queue>,
    /// Access counter for LRU tracking
    access_counter: u64,
    /// Number of cache hits (for stats)
    cache_hits: u64,
    /// Number of cache misses (for stats)
    cache_misses: u64,
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
            format: wgpu::TextureFormat::R8Unorm,
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

        Self {
            texture,
            _view: view,
            bind_group,
            bind_group_layout,
            cache: HashMap::with_capacity(1024),
            swash_cache: SwashCache::new(),
            rows: Vec::new(),
            current_y: 0,
            queue,
            access_counter: 0,
            cache_hits: 0,
            cache_misses: 0,
        }
    }

    /// Get the bind group layout.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    /// Get the bind group.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Get or cache a glyph.
    pub fn get_glyph(
        &mut self,
        font_system: &mut cosmic_text::FontSystem,
        cache_key: CacheKey,
    ) -> Option<GlyphInfo> {
        // Check cache first
        if let Some(entry) = self.cache.get_mut(&cache_key) {
            self.access_counter += 1;
            entry.last_access = self.access_counter;
            self.cache_hits += 1;
            return Some(entry.info);
        }

        self.cache_misses += 1;

        // Check if we need to evict before adding
        if self.cache.len() >= MAX_CACHED_GLYPHS {
            self.evict_lru();
        }

        // Rasterize the glyph using uncached method to get owned data
        let image = self
            .swash_cache
            .get_image_uncached(font_system, cache_key)?;

        // Process the image
        self.process_image(cache_key, image)
    }

    /// Evict glyphs to make room in the atlas.
    ///
    /// Resetting atlas allocation invalidates all UV coordinates, so we must
    /// clear the entire cache. Glyphs that are still needed will be
    /// re-rasterized and re-uploaded on next access.
    fn evict_lru(&mut self) {
        let evicted = self.cache.len();
        self.cache.clear();
        self.rows.clear();
        self.current_y = 0;

        tracing::debug!("Evicted all {} glyphs from atlas", evicted);
    }

    /// Process a rasterized glyph image.
    fn process_image(&mut self, cache_key: CacheKey, image: SwashImage) -> Option<GlyphInfo> {
        self.access_counter += 1;

        // Skip empty glyphs
        if image.placement.width == 0 || image.placement.height == 0 {
            // Return a zero-size glyph info
            let info = GlyphInfo {
                uv: [0.0, 0.0, 0.0, 0.0],
                width: 0.0,
                height: 0.0,
                left: image.placement.left as f32,
                top: image.placement.top as f32,
            };
            self.cache.insert(cache_key, CacheEntry {
                info,
                last_access: self.access_counter,
            });
            return Some(info);
        }

        // Find space in the atlas
        let (x, y) = self.allocate(image.placement.width, image.placement.height)?;

        // Convert image data based on content type
        let data: Vec<u8> = match image.content {
            SwashContent::Mask => image.data.clone(),
            SwashContent::Color => {
                // Convert RGBA to grayscale (use alpha channel)
                image.data.chunks(4).map(|c| c[3]).collect()
            }
            SwashContent::SubpixelMask => {
                // Use red channel for subpixel
                image.data.chunks(3).map(|c| c[0]).collect()
            }
        };

        // Upload to texture
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(image.placement.width),
                rows_per_image: Some(image.placement.height),
            },
            wgpu::Extent3d {
                width: image.placement.width,
                height: image.placement.height,
                depth_or_array_layers: 1,
            },
        );

        // Calculate UV coordinates
        let atlas_size = ATLAS_SIZE as f32;
        let info = GlyphInfo {
            uv: [
                x as f32 / atlas_size,
                y as f32 / atlas_size,
                (x + image.placement.width) as f32 / atlas_size,
                (y + image.placement.height) as f32 / atlas_size,
            ],
            width: image.placement.width as f32,
            height: image.placement.height as f32,
            left: image.placement.left as f32,
            top: image.placement.top as f32,
        };

        self.cache.insert(cache_key, CacheEntry {
            info,
            last_access: self.access_counter,
        });
        Some(info)
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
            // Atlas is full - in production we'd create a new atlas or evict
            tracing::warn!("Glyph atlas full, some glyphs may not render");
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

    /// Clear the atlas cache.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.rows.clear();
        self.current_y = 0;
    }

    /// Get cache statistics.
    pub fn stats(&self) -> AtlasStats {
        AtlasStats {
            cached_glyphs: self.cache.len(),
            atlas_usage: self.current_y,
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
        }
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

    #[test]
    fn glyph_info_layout() {
        let info = GlyphInfo {
            uv: [0.1, 0.2, 0.3, 0.4],
            width: 10.0,
            height: 20.0,
            left: 1.0,
            top: 15.0,
        };

        assert_eq!(info.uv[0], 0.1);
        assert_eq!(info.width, 10.0);
        assert_eq!(info.top, 15.0);
    }
}
