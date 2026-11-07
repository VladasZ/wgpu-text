use std::ops::Range;

use glyph_brush::{
    Rectangle,
    ab_glyph::{Rect, point},
};

use crate::{Matrix, cache::Cache, shared::Shared};

/// Responsible for drawing text.
#[derive(Debug)]
pub struct Pipeline {
    inner: wgpu::RenderPipeline,
    /// Draws the sections with a spread, see `TextExtra::spread`.
    effect: wgpu::RenderPipeline,
    cache: Cache,

    vertex_buffer: wgpu::Buffer,
    /// Where the next vertex write of this frame goes.
    cursor: u64,
    /// Bytes of the last written vertices.
    range: Range<u64>,
    vertices: u32,
    /// How many of the vertices, from the front, are effect glyphs.
    effects: u32,

    /// Vertices of every batch of this frame, to know which glyphs of the
    /// cache texture are still drawn.
    batches: Vec<Vec<Vertex>>,
    /// The batches of a frame did not fit the cache texture together.
    grow_cache: bool,
}

impl Pipeline {
    pub(crate) fn new(
        device: &wgpu::Device,
        shared: Shared,
        tex_dimensions: (u32, u32),
        matrix: Matrix,
    ) -> Pipeline {
        let cache = Cache::new(
            device,
            tex_dimensions,
            matrix,
            shared.bind_group_layout.clone(),
        );

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wgpu-text Vertex Buffer"),
            size: 0,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            inner: shared.inner,
            effect: shared.effect,
            cache,

            vertex_buffer,
            cursor: 0,
            range: 0..0,
            vertices: 0,
            effects: 0,

            batches: Vec::new(),
            grow_cache: false,
        }
    }

    /// Raw draw.
    pub fn draw(&self, rpass: &mut wgpu::RenderPass) {
        if self.vertices != 0 {
            rpass.set_vertex_buffer(0, self.vertex_buffer.slice(self.range.clone()));
            rpass.set_bind_group(0, &self.cache.bind_group, &[]);

            // The outlines and shadows first, the text they sit behind
            // blends over them.
            if self.effects != 0 {
                rpass.set_pipeline(&self.effect);
                rpass.draw(0..4, 0..self.effects);
            }
            if self.effects != self.vertices {
                rpass.set_pipeline(&self.inner);
                rpass.draw(0..4, self.effects..self.vertices);
            }
        }
    }

    /// `wgpu` runs all buffer writes before any render pass, so an appending
    /// write goes after the earlier batches and a plain one starts over.
    pub fn update_vertex_buffer(
        &mut self,
        vertices: Vec<Vertex>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        append: bool,
    ) {
        if !append {
            self.cursor = 0;
            self.batches.clear();
        }

        // The effect glyphs go to the front of the batch, each kind keeps
        // its order, so the 2 pipelines draw 2 ranges of 1 buffer.
        let mut vertices = vertices;
        vertices.sort_by_key(|vertex| !vertex.is_effect());
        self.effects = vertices
            .iter()
            .take_while(|vertex| vertex.is_effect())
            .count() as u32;

        self.vertices = vertices.len() as u32;

        if vertices.is_empty() {
            self.range = self.cursor..self.cursor;
            self.batches.push(vertices);
            return;
        }

        let data: &[u8] = bytemuck::cast_slice(&vertices);
        let size = data.len() as u64;

        if self.cursor + size > self.vertex_buffer.size() {
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wgpu-text Vertex Buffer"),
                size: size.max(self.vertex_buffer.size() * 2).max(4096),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.cursor = 0;
        }

        queue.write_buffer(&self.vertex_buffer, self.cursor, data);
        self.range = self.cursor..self.cursor + size;
        self.cursor = self.range.end.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        self.batches.push(vertices);
    }

    /// Draws the last vertices again, a later appending write goes after them.
    pub fn redraw(&mut self, append: bool) {
        let end = self.range.end.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        self.cursor = if append { self.cursor.max(end) } else { end };

        if !append {
            let earlier = self.batches.len().saturating_sub(1);
            self.batches.drain(..earlier);
        }
    }

    /// Texture writes run before any render pass too, and a full cache reuses
    /// the space of glyphs the batch does not need. If an earlier batch still
    /// draws one of them, the writes go into a copy of the texture.
    pub fn update_texture_append(
        &mut self,
        writes: &[(Rectangle<u32>, Vec<u8>)],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        if writes.iter().any(|(rect, _)| self.is_drawn(rect)) {
            self.cache.fork_texture(device, queue);
            self.batches.clear();
            self.grow_cache = true;
        }

        for (rect, data) in writes {
            self.cache.update_texture(*rect, data, queue);
        }
    }

    /// Whether a write to `rect` lands in the atlas slot of a glyph that a
    /// batch of this frame draws.
    ///
    /// The test uses the slot, not the drawn quad. The quad is grown by the
    /// stem darkening or the spread, and the cache packs the next glyph
    /// right after the slot of the last one. A test on the quad took every
    /// new glyph in a row for an overwrite, and each of those copied and
    /// doubled the whole atlas.
    fn is_drawn(&self, rect: &Rectangle<u32>) -> bool {
        let dimensions = self.cache.texture_dimensions();
        self.batches
            .iter()
            .flatten()
            .filter_map(|vertex| vertex.atlas_slot(dimensions))
            .any(|slot| slot.overlaps(rect))
    }

    /// The cache texture size to start the frame with, if it has to grow.
    pub fn take_cache_growth(&mut self, max_dimension: u32) -> Option<(u32, u32)> {
        if !std::mem::take(&mut self.grow_cache) {
            return None;
        }

        let (width, height) = self.cache.texture_dimensions();
        let grown = (
            (width * 2).min(max_dimension),
            (height * 2).min(max_dimension),
        );
        (grown != (width, height)).then_some(grown)
    }

    #[inline]
    pub fn update_matrix(&self, matrix: Matrix, queue: &wgpu::Queue) {
        self.cache.update_matrix(matrix, queue);
    }

    #[inline]
    pub fn update_texture(
        &mut self,
        size: Rectangle<u32>,
        data: &[u8],
        queue: &wgpu::Queue,
    ) {
        self.cache.update_texture(size, data, queue);
    }

    #[inline]
    pub fn resize_texture(&mut self, device: &wgpu::Device, tex_dimensions: (u32, u32)) {
        self.cache.recreate_texture(device, tex_dimensions);
        // Recorded draws keep the old texture.
        self.batches.clear();
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    top_left: [f32; 3],
    bottom_right: [f32; 2],
    tex_top_left: [f32; 2],
    tex_bottom_right: [f32; 2],
    color: [f32; 4],
    /// Top and bottom of the section box, the range the gradient ramps over.
    ramp: [f32; 2],
    /// Packed to keep the per glyph cost at 12 bytes instead of 24. Eight bits
    /// per channel is what a source color has anyway.
    end_color: [u8; 4],
    /// The glyph's own box before the spread grew the quad, left, top,
    /// right, bottom. The effect shader takes no sample outside it, the
    /// atlas has other glyphs there.
    glyph_rect: [f32; 4],
    /// Pixels the coverage spreads by, and 1 when the spread is a blur.
    /// Both 0 for plain text.
    spread: [f32; 2],
}

/// The shader reads this back as `Unorm8x4`.
fn pack_color(color: [f32; 4]) -> [u8; 4] {
    color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

/// Texels of the atlas, the max side not included.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    min: [f32; 2],
    max: [f32; 2],
}

impl Slot {
    fn overlaps(&self, rect: &Rectangle<u32>) -> bool {
        self.min[0] < rect.max[0] as f32
            && self.max[0] > rect.min[0] as f32
            && self.min[1] < rect.max[1] as f32
            && self.max[1] > rect.min[1] as f32
    }
}

impl Vertex {
    /// Drawn by the effect pipeline, an outline or a soft shadow.
    pub fn is_effect(&self) -> bool {
        self.spread[0] > 0.0
    }

    /// The texels the cache gave this glyph: its own box and the 1 texel of
    /// padding around it, the most any shader reads. The quad maps to its
    /// texture window in a straight line, so the box in pixels maps back to
    /// the box in the atlas, also for a quad cut at the section bounds.
    /// `None` for a quad cut to nothing.
    fn atlas_slot(&self, (width, height): (u32, u32)) -> Option<Slot> {
        let quad = [
            self.bottom_right[0] - self.top_left[0],
            self.bottom_right[1] - self.top_left[1],
        ];
        if quad[0] <= 0.0 || quad[1] <= 0.0 {
            return None;
        }

        let size = [width as f32, height as f32];
        let edge = |axis: usize, pixel: f32| {
            let texels_per_pixel =
                (self.tex_bottom_right[axis] - self.tex_top_left[axis]) * size[axis]
                    / quad[axis];
            self.tex_top_left[axis] * size[axis]
                + (pixel - self.top_left[axis]) * texels_per_pixel
        };
        let [left, top, right, bottom] = self.glyph_rect;

        // The cache places glyphs on whole texels, rounding drops the float
        // error so a glyph packed right after this slot does not touch it.
        Some(Slot {
            min: [edge(0, left).round() - 1.0, edge(1, top).round() - 1.0],
            max: [edge(0, right).round() + 1.0, edge(1, bottom).round() + 1.0],
        })
    }

    /// The glyph quad and its texture window grown by `stem_px` on every
    /// side, so the darkening entry point's dilation taps have room to
    /// draw the widened edge instead of clipping it at the outline. The
    /// atlas pads glyphs, so a sub pixel growth reads blank space, not a
    /// neighbor glyph. A section with a spread grows by the spread instead,
    /// and carries the glyph's own box so the effect shader stays inside it.
    pub fn to_vertex_inflated(
        glyph_brush::GlyphVertex {
            mut tex_coords,
            pixel_coords,
            bounds,
            extra,
        }: glyph_brush::GlyphVertex<crate::TextExtra>,
        stem_px: f32,
        tex_dimensions: (u32, u32),
    ) -> Vertex {
        let spread = extra.spread.clamp(0.0, crate::MAX_SPREAD);
        // The blur reaches a pixel past its radius, the widening half a
        // pixel for its smooth edge. A whole pixel keeps texels on pixels.
        let stem_px = if spread > 0.0 {
            spread.ceil() + 1.0
        } else {
            stem_px
        };
        let glyph_rect = [
            pixel_coords.min.x,
            pixel_coords.min.y,
            pixel_coords.max.x,
            pixel_coords.max.y,
        ];

        let mut rect = Rect {
            min: point(pixel_coords.min.x - stem_px, pixel_coords.min.y - stem_px),
            max: point(pixel_coords.max.x + stem_px, pixel_coords.max.y + stem_px),
        };

        let tex_dx = stem_px / tex_dimensions.0 as f32;
        let tex_dy = stem_px / tex_dimensions.1 as f32;
        tex_coords.min.x -= tex_dx;
        tex_coords.min.y -= tex_dy;
        tex_coords.max.x += tex_dx;
        tex_coords.max.y += tex_dy;

        // handle overlapping bounds, modify uv_rect to preserve texture aspect
        if rect.max.x > bounds.max.x {
            let old_width = rect.width();
            rect.max.x = bounds.max.x;
            tex_coords.max.x =
                tex_coords.min.x + tex_coords.width() * rect.width() / old_width;
        }
        if rect.min.x < bounds.min.x {
            let old_width = rect.width();
            rect.min.x = bounds.min.x;
            tex_coords.min.x =
                tex_coords.max.x - tex_coords.width() * rect.width() / old_width;
        }
        if rect.max.y > bounds.max.y {
            let old_height = rect.height();
            rect.max.y = bounds.max.y;
            tex_coords.max.y =
                tex_coords.min.y + tex_coords.height() * rect.height() / old_height;
        }
        if rect.min.y < bounds.min.y {
            let old_height = rect.height();
            rect.min.y = bounds.min.y;
            tex_coords.min.y =
                tex_coords.max.y - tex_coords.height() * rect.height() / old_height;
        }

        Vertex {
            top_left: [rect.min.x, rect.min.y, extra.z],
            bottom_right: [rect.max.x, rect.max.y],
            tex_top_left: [tex_coords.min.x, tex_coords.min.y],
            tex_bottom_right: [tex_coords.max.x, tex_coords.max.y],
            color: extra.color,
            ramp: [bounds.min.y, bounds.max.y],
            end_color: pack_color(extra.end_color),
            glyph_rect,
            spread: [spread, if extra.soft { 1.0 } else { 0.0 }],
        }
    }

    pub fn buffer_layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: std::mem::size_of::<[f32; 5]>() as wgpu::BufferAddress,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: std::mem::size_of::<[f32; 7]>() as wgpu::BufferAddress,
                    shader_location: 3,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: std::mem::size_of::<[f32; 9]>() as wgpu::BufferAddress,
                    shader_location: 4,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: std::mem::size_of::<[f32; 13]>() as wgpu::BufferAddress,
                    shader_location: 5,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Unorm8x4,
                    offset: std::mem::size_of::<[f32; 15]>() as wgpu::BufferAddress,
                    shader_location: 6,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: std::mem::size_of::<[f32; 16]>() as wgpu::BufferAddress,
                    shader_location: 7,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: std::mem::size_of::<[f32; 20]>() as wgpu::BufferAddress,
                    shader_location: 8,
                },
            ],
        }
    }
}

#[cfg(test)]
mod test {
    use glyph_brush::{
        GlyphVertex, Rectangle,
        ab_glyph::{Rect, point},
    };

    use super::{Slot, Vertex};
    use crate::TextExtra;

    const ATLAS: (u32, u32) = (256, 256);

    /// A 10 by 10 glyph the cache placed at texels 11 to 21, so its padded
    /// slot is 10 to 22, drawn at pixels 100 to 110.
    fn glyph(extra: &TextExtra, stem_px: f32, bounds: Rect) -> Vertex {
        let texel = |value: f32| value / ATLAS.0 as f32;
        Vertex::to_vertex_inflated(
            GlyphVertex {
                tex_coords: Rect {
                    min: point(texel(11.0), texel(11.0)),
                    max: point(texel(21.0), texel(21.0)),
                },
                pixel_coords: Rect {
                    min: point(100.0, 100.0),
                    max: point(110.0, 110.0),
                },
                bounds,
                extra,
            },
            stem_px,
            ATLAS,
        )
    }

    fn open_bounds() -> Rect {
        Rect {
            min: point(0.0, 0.0),
            max: point(1000.0, 1000.0),
        }
    }

    fn rect(min: [u32; 2], max: [u32; 2]) -> Rectangle<u32> {
        Rectangle { min, max }
    }

    const PADDED_SLOT: Slot = Slot {
        min: [10.0, 10.0],
        max: [22.0, 22.0],
    };

    #[test]
    fn a_darkened_glyph_owns_its_padded_slot_only() {
        let extra = TextExtra::flat([1.0; 4], 0.0);
        let slot = glyph(&extra, 0.5, open_bounds()).atlas_slot(ATLAS).unwrap();

        assert_eq!(slot, PADDED_SLOT);
        // The cache packs the next glyph of the row right after the slot.
        assert!(!slot.overlaps(&rect([22, 10], [34, 22])));
        // A glyph in a row freed by the cache lands on the slot.
        assert!(slot.overlaps(&rect([12, 12], [20, 20])));
        assert!(slot.overlaps(&rect([0, 21], [11, 30])));
    }

    #[test]
    fn an_outline_owns_the_same_slot_as_its_glyph() {
        let mut extra = TextExtra::flat([1.0; 4], 0.0);
        extra.spread = 3.0;
        let vertex = glyph(&extra, 0.0, open_bounds());

        assert!(vertex.is_effect());
        assert_eq!(vertex.atlas_slot(ATLAS), Some(PADDED_SLOT));
    }

    #[test]
    fn a_glyph_cut_by_its_section_keeps_its_whole_slot() {
        let extra = TextExtra::flat([1.0; 4], 0.0);
        let bounds = Rect {
            min: point(0.0, 0.0),
            max: point(104.0, 1000.0),
        };

        assert_eq!(
            glyph(&extra, 0.5, bounds).atlas_slot(ATLAS),
            Some(PADDED_SLOT)
        );
    }
}
