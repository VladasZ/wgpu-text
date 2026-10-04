use glyph_brush::Rectangle;
use wgpu::util::DeviceExt;

use crate::Matrix;

/// Responsible for texture caching and the global matrix.
#[derive(Debug)]
pub struct Cache {
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,

    matrix_buffer: wgpu::Buffer,
    texture: wgpu::Texture,
    sampler: wgpu::Sampler,

    /// What the texture holds, one byte a texel. A fork writes the new
    /// texture from here. A texture to texture copy made an LG TV with a
    /// Mali GPU and the Chromium 79 engine drop the whole WebGL context.
    pixels: Vec<u8>,
}

impl Cache {
    pub fn new(
        device: &wgpu::Device,
        tex_dimensions: (u32, u32),
        matrix: Matrix,
        bind_group_layout: wgpu::BindGroupLayout,
    ) -> Self {
        let texture = Self::create_cache_texture(device, tex_dimensions);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wgpu-text Cache Texture Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let matrix_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("wgpu-text Matrix Buffer"),
                contents: bytemuck::cast_slice(&matrix),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wgpu-text Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: matrix_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            matrix_buffer,
            texture,
            sampler,
            bind_group,
            bind_group_layout,
            pixels: empty_pixels(tex_dimensions),
        }
    }

    pub fn recreate_texture(
        &mut self,
        device: &wgpu::Device,
        tex_dimensions: (u32, u32),
    ) {
        self.set_texture(device, Self::create_cache_texture(device, tex_dimensions));
        self.pixels = empty_pixels(tex_dimensions);
    }

    /// Replaces the texture with a copy of itself. Draws that are already
    /// recorded keep the old texture, so later writes do not reach them.
    pub fn fork_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let (width, height) = self.texture_dimensions();
        let texture = Self::create_cache_texture(device, (width, height));
        queue.write_texture(
            texture.as_image_copy(),
            &self.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            texture.size(),
        );
        self.set_texture(device, texture);
    }

    pub fn texture_dimensions(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }

    fn set_texture(&mut self, device: &wgpu::Device, texture: wgpu::Texture) {
        self.texture = texture;
        self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wgpu-text Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.matrix_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &self
                            .texture
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
    }

    pub fn update_matrix(&self, matrix: Matrix, queue: &wgpu::Queue) {
        queue.write_buffer(&self.matrix_buffer, 0, bytemuck::cast_slice(&matrix));
    }

    pub fn update_texture(
        &mut self,
        size: Rectangle<u32>,
        data: &[u8],
        queue: &wgpu::Queue,
    ) {
        write_pixels(&mut self.pixels, self.texture.width(), size, data);

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: size.min[0],
                    y: size.min[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width()),
                rows_per_image: Some(size.height()),
            },
            wgpu::Extent3d {
                width: size.width(),
                height: size.height(),
                depth_or_array_layers: 1,
            },
        )
    }

    fn create_cache_texture(
        device: &wgpu::Device,
        dimensions: (u32, u32),
    ) -> wgpu::Texture {
        let size = wgpu::Extent3d {
            width: dimensions.0,
            height: dimensions.1,
            depth_or_array_layers: 1,
        };
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wgpu-text Cache Texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }
}

/// The layout of the matrix, the cache texture and its sampler. The same
/// for every brush, so the shared pipelines carry one, see `shared.rs`.
pub(crate) fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("wgpu-text Matrix, Texture and Sampler Bind Group Layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(
                        std::mem::size_of::<Matrix>() as wgpu::BufferAddress,
                    ),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn empty_pixels(dimensions: (u32, u32)) -> Vec<u8> {
    vec![0; dimensions.0 as usize * dimensions.1 as usize]
}

/// Copies the rows of `data` into `rect` of a texture `width` texels wide.
fn write_pixels(pixels: &mut [u8], width: u32, rect: Rectangle<u32>, data: &[u8]) {
    let width = width as usize;
    let rect_width = rect.width() as usize;
    let left = rect.min[0] as usize;

    for (row, line) in data.chunks_exact(rect_width).enumerate() {
        let start = (rect.min[1] as usize + row) * width + left;
        pixels[start..start + rect_width].copy_from_slice(line);
    }
}

#[cfg(test)]
mod test {
    use glyph_brush::Rectangle;

    use super::{empty_pixels, write_pixels};

    #[test]
    fn a_write_lands_in_its_rect_only() {
        let mut pixels = empty_pixels((4, 3));
        let rect = Rectangle {
            min: [1, 1],
            max: [3, 3],
        };

        write_pixels(&mut pixels, 4, rect, &[1, 2, 3, 4]);

        #[rustfmt::skip]
        assert_eq!(pixels, [
            0, 0, 0, 0,
            0, 1, 2, 0,
            0, 3, 4, 0,
        ]);
    }
}
