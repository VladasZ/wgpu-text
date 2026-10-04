//! The pipelines of a brush, built once for every brush that draws into the
//! same kind of target.
//!
//! A brush used to build its own shader module and pipelines. An app makes
//! one brush per font, and nothing in the pipelines depends on the font. On
//! an LG TV each of the 22 fonts of an app paid about 1.2 seconds for them
//! at start, 23 of the 28 seconds of its boot.

use std::{cell::RefCell, num::NonZeroU32};

use crate::{cache::create_bind_group_layout, pipeline::Vertex};

#[derive(Clone, Debug)]
pub(crate) struct Shared {
    pub(crate) inner: wgpu::RenderPipeline,
    /// Draws the sections with a spread, see `TextExtra::spread`.
    pub(crate) effect: wgpu::RenderPipeline,
    pub(crate) bind_group_layout: wgpu::BindGroupLayout,
}

/// Everything a pipeline is built from.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    device: wgpu::Device,
    render_format: wgpu::TextureFormat,
    depth_stencil: Option<wgpu::DepthStencilState>,
    multisample: wgpu::MultisampleState,
    multiview_mask: Option<NonZeroU32>,
    stem_darkening: u32,
}

thread_local! {
    // The gpu handles are not `Send` in a browser, and brushes are built
    // on the thread that draws.
    static BUILT: RefCell<Vec<(Key, Shared)>> = const { RefCell::new(Vec::new()) };
}

impl Shared {
    pub(crate) fn get(
        device: &wgpu::Device,
        render_format: wgpu::TextureFormat,
        depth_stencil: Option<wgpu::DepthStencilState>,
        multisample: wgpu::MultisampleState,
        multiview_mask: Option<NonZeroU32>,
        stem_darkening: f32,
    ) -> Self {
        let key = Key {
            device: device.clone(),
            render_format,
            depth_stencil,
            multisample,
            multiview_mask,
            stem_darkening: stem_darkening.to_bits(),
        };

        BUILT.with_borrow_mut(|built| {
            if let Some((_, shared)) = built.iter().find(|(known, _)| *known == key) {
                return shared.clone();
            }

            let shared = Self::build(&key, stem_darkening);
            built.push((key, shared.clone()));
            shared
        })
    }

    fn build(key: &Key, stem_darkening: f32) -> Self {
        let device = &key.device;
        let render_format = key.render_format;
        let depth_stencil = key.depth_stencil.clone();
        let multisample = key.multisample;
        let multiview_mask = key.multiview_mask;

        let bind_group_layout = create_bind_group_layout(device);

        let depth_stencil_for_effect = depth_stencil.clone().map(|mut depth| {
            depth.depth_compare = Some(wgpu::CompareFunction::LessEqual);
            depth
        });

        let shader =
            device.create_shader_module(wgpu::include_wgsl!("shader/shader.wgsl"));

        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("wgpu-text Render Pipeline Layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });

        // Stem darkening replaces the plain entry, see the shader.
        let fs_entry = if stem_darkening > 0.0 {
            "fs_main_darken"
        } else {
            "fs_main"
        };

        // Only the darkening entry point reads the override. WebKit builds
        // the Metal fragment function with the constants it is given, and a
        // value for an override the entry point never references fails that
        // build with "Fragment library could not be created", so the plain
        // entry gets none.
        let constants: &[(&str, f64)] = if fs_entry == "fs_main_darken" {
            &[("stem_px", f64::from(stem_darkening))]
        } else {
            &[]
        };

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
                    constants,
                    ..Default::default()
                },
            }),
            cache: None,
            multiview_mask,
        });

        // Sections with a spread, the outlines and soft shadows. Their own
        // entry points, the plain ones have no room for what the spread
        // needs, see the shader. Their quads are grown by the spread and
        // overlap the quads of the glyphs next to them at the same depth,
        // so an equal depth must pass, or a glyph would cut the outline of
        // its neighbor.
        let effect_depth = depth_stencil_for_effect;
        let effect = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wgpu-text Effect Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_effect"),
                buffers: &[Some(Vertex::buffer_layout())],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: Some(wgpu::IndexFormat::Uint16),
                ..Default::default()
            },
            depth_stencil: effect_depth,
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_effect"),
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
            effect,
            bind_group_layout,
        }
    }
}
