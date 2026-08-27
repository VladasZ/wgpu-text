use std::{num::NonZeroU32, ops::Range};

use glyph_brush::{
    Rectangle,
    ab_glyph::{Rect, point},
};

use crate::{Matrix, cache::Cache};

/// Responsible for drawing text.
#[derive(Debug)]
pub struct Pipeline {
    inner: wgpu::RenderPipeline,
    cache: Cache,

    vertex_buffer: wgpu::Buffer,
    /// Where the next vertex write of this frame goes.
    cursor: u64,
    /// Bytes of the last written vertices.
    range: Range<u64>,
    vertices: u32,

    /// Vertices of every batch of this frame, to know which glyphs of the
    /// cache texture are still drawn.
    batches: Vec<Vec<Vertex>>,
    /// The batches of a frame did not fit the cache texture together.
    grow_cache: bool,
}

impl Pipeline {
    pub fn new(
        device: &wgpu::Device,
        render_format: wgpu::TextureFormat,
        depth_stencil: Option<wgpu::DepthStencilState>,
        multisample: wgpu::MultisampleState,
        multiview_mask: Option<NonZeroU32>,
        tex_dimensions: (u32, u32),
        matrix: Matrix,
        stem_darkening: f32,
    ) -> Pipeline {
        let cache = Cache::new(device, tex_dimensions, matrix);

        let shader =
            device.create_shader_module(wgpu::include_wgsl!("shader/shader.wgsl"));

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wgpu-text Vertex Buffer"),
            size: 0,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("wgpu-text Render Pipeline Layout"),
                bind_group_layouts: &[Some(&cache.bind_group_layout)],
                immediate_size: 0,
            });

        // Stem darkening replaces the plain entry, see the shader.
        let fs_entry = if stem_darkening > 0.0 {
            "fs_main_darken"
        } else {
            "fs_main"
        };

        let constants = [("stem_px", f64::from(stem_darkening))];

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wgpu-text Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(Vertex::buffer_layout())],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: Some(wgpu::IndexFormat::Uint16),
                ..Default::default()
            },
            depth_stencil,
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fs_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
            }),
            cache: None,
            multiview_mask,
        });

        Self {
            inner: pipeline,
            cache,

            vertex_buffer,
            cursor: 0,
            range: 0..0,
            vertices: 0,

            batches: Vec::new(),
            grow_cache: false,
        }
    }

    /// Raw draw.
    pub fn draw(&self, rpass: &mut wgpu::RenderPass) {
        if self.vertices != 0 {
            rpass.set_pipeline(&self.inner);
            rpass.set_vertex_buffer(0, self.vertex_buffer.slice(self.range.clone()));
            rpass.set_bind_group(0, &self.cache.bind_group, &[]);

            rpass.draw(0..4, 0..self.vertices);
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

    /// Whether a batch of this frame draws a glyph from `rect`, with 1 texel
    /// around it for the linear filter.
    fn is_drawn(&self, rect: &Rectangle<u32>) -> bool {
        let (width, height) = self.cache.texture_dimensions();
        let (width, height) = (width as f32, height as f32);
        let min = [rect.min[0] as f32, rect.min[1] as f32];
        let max = [rect.max[0] as f32, rect.max[1] as f32];

        self.batches.iter().flatten().any(|v| {
            v.tex_top_left[0] * width - 1.0 < max[0]
                && v.tex_bottom_right[0] * width + 1.0 > min[0]
                && v.tex_top_left[1] * height - 1.0 < max[1]
                && v.tex_bottom_right[1] * height + 1.0 > min[1]
        })
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
    pub fn update_texture(&self, size: Rectangle<u32>, data: &[u8], queue: &wgpu::Queue) {
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
}

/// The shader reads this back as `Unorm8x4`.
fn pack_color(color: [f32; 4]) -> [u8; 4] {
    color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

impl Vertex {
    /// The glyph quad and its texture window grown by `stem_px` on every
    /// side, so the darkening entry point's dilation taps have room to
    /// draw the widened edge instead of clipping it at the outline. The
    /// atlas pads glyphs, so a sub pixel growth reads blank space, not a
    /// neighbor glyph.
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
            ],
        }
    }
}
