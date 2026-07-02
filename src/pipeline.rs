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
    /// Bump allocation cursor for this frame's vertex writes.
    cursor: u64,
    /// Byte range of the last written vertices, used by `draw`.
    range: Range<u64>,
    vertices: u32,
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

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wgpu-text Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::buffer_layout()],
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
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
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

    /// All queued buffer writes execute together before any render pass,
    /// so a second write in the same frame must not touch bytes an earlier
    /// recorded draw still reads. The buffer is bump-allocated through the
    /// frame and `next_frame` resets the cursor. Draws recorded with the
    /// old buffer keep it alive when it is replaced here.
    pub fn update_vertex_buffer(
        &mut self,
        vertices: Vec<Vertex>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        self.vertices = vertices.len() as u32;

        if vertices.is_empty() {
            self.range = self.cursor..self.cursor;
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
    }

    /// Keeps the previous vertices for another draw. The cursor still
    /// skips past them so a later write in the same frame does not
    /// overwrite what the reused draw reads.
    pub fn redraw(&mut self) {
        self.cursor = self
            .cursor
            .max(self.range.end.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT));
    }

    /// Marks the start of a new frame. Earlier writes are no longer
    /// referenced by recorded draws, so the buffer is reused from the
    /// start. Without this the cursor only moves forward and the buffer
    /// grows without bound.
    pub fn next_frame(&mut self) {
        self.cursor = 0;
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
}

impl Vertex {
    pub fn to_vertex(
        glyph_brush::GlyphVertex {
            mut tex_coords,
            pixel_coords,
            bounds,
            extra,
        }: glyph_brush::GlyphVertex,
    ) -> Vertex {
        let mut rect = Rect {
            min: point(pixel_coords.min.x, pixel_coords.min.y),
            max: point(pixel_coords.max.x, pixel_coords.max.y),
        };

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
            ],
        }
    }
}
