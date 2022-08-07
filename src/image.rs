use crate::renderer::{Align, DEFAULT_MARGIN};
use crate::InlyneEvent;
use bytemuck::{Pod, Zeroable};
use image::RgbaImage;
use std::fs::File;
use std::io::Read;
use std::sync::{Arc, Mutex, MutexGuard};
use wgpu::util::DeviceExt;
use wgpu::{Device, TextureFormat};
use winit::event_loop::EventLoopProxy;

use std::borrow::Cow;

#[derive(Debug)]
pub enum ImageSize {
    PxWidth(u32),
    PxHeight(u32),
    FullSize((u32, u32)),
}

pub struct Image {
    image: Arc<Mutex<Option<RgbaImage>>>,
    pub is_aligned: Option<Align>,
    callback: Arc<Mutex<Option<EventLoopProxy<InlyneEvent>>>>,
    pub size: Option<ImageSize>,
    pub bind_group: Option<Arc<wgpu::BindGroup>>,
}

impl Image {
    pub fn create_bind_group(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &wgpu::Sampler,
        bindgroup_layout: &wgpu::BindGroupLayout,
    ) {
        let dimensions = self.buffer_dimensions();
        if let Some(image_data) = self.image.lock().unwrap().as_ref() {
            let texture_size = wgpu::Extent3d {
                width: dimensions.0,
                height: dimensions.1,
                depth_or_array_layers: 1,
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                // All textures are stored as 3D, we represent our 2D texture
                // by setting depth to 1.
                size: texture_size,
                mip_level_count: 1, // We'll talk about this a little later
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // Most images are stored using sRGB so we need to reflect that here.
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                // TEXTURE_BINDING tells wgpu that we want to use this texture in shaders
                // COPY_DST means that we want to copy data to this texture
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                label: Some("diffuse_texture"),
            });
            queue.write_texture(
                // Tells wgpu where to copy the pixel data
                wgpu::ImageCopyTexture {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                // The actual pixel data
                image_data,
                // The layout of the texture
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: std::num::NonZeroU32::new(4 * dimensions.0),
                    rows_per_image: std::num::NonZeroU32::new(dimensions.1),
                },
                texture_size,
            );

            let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: bindgroup_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
                label: Some("diffuse_bind_group"),
            });
            self.bind_group = Some(Arc::new(bind_group));
        }
    }

    pub fn from_url(url: String) -> Image {
        let image = Arc::new(Mutex::new(None));
        let callback = Arc::new(Mutex::new(None::<EventLoopProxy<InlyneEvent>>));
        let image_clone = image.clone();
        let callback_clone = callback.clone();
        std::thread::spawn(move || {
            let image_data = if let Ok(mut img_file) = File::open(url.as_str()) {
                let img_file_size = std::fs::metadata(url.as_str()).unwrap().len();
                let mut img_buf = Vec::with_capacity(img_file_size as usize);
                img_file.read_to_end(&mut img_buf).unwrap();
                img_buf
            } else if let Ok(Ok(data)) = reqwest::blocking::get(url).map(|request| request.bytes())
            {
                data.into()
            } else {
                return;
            };

            if let Ok(image) = image::load_from_memory(&image_data) {
                *(image_clone.lock().unwrap()) = Some(image.to_rgba8());
            }
            if let Ok(Some(callback)) = callback_clone.try_lock().as_deref() {
                callback.send_event(InlyneEvent::Reposition).unwrap();
            }
        });

        Image {
            image,
            is_aligned: None,
            callback,
            size: None,
            bind_group: None,
        }
    }

    pub fn add_callback(&mut self, eventloop_proxy: EventLoopProxy<InlyneEvent>) {
        *(self.callback.lock().unwrap()) = Some(eventloop_proxy);
    }

    pub fn with_align(mut self, align: Align) -> Self {
        self.is_aligned = Some(align);
        self
    }
    pub fn with_size(mut self, size: ImageSize) -> Self {
        self.size = Some(size);
        self
    }
    pub fn dimensions_from_image_size(&self, size: &ImageSize) -> (u32, u32) {
        let image_dimensions = self.buffer_dimensions();
        match size {
            ImageSize::PxWidth(px_width) => (
                *px_width,
                ((*px_width as f32 / image_dimensions.0 as f32) * image_dimensions.1 as f32) as u32,
            ),
            ImageSize::PxHeight(px_height) => (
                ((*px_height as f32 / image_dimensions.1 as f32) * image_dimensions.0 as f32)
                    as u32,
                *px_height,
            ),
            ImageSize::FullSize((px_width, px_height)) => (*px_width, *px_height),
        }
    }

    pub fn buffer_dimensions(&self) -> (u32, u32) {
        if let Ok(Some(image)) = self.image.try_lock().as_deref() {
            image.dimensions()
        } else {
            (0, 0)
        }
    }
    pub fn dimensions(&self, hidpi_scale: f32, screen_size: (f32, f32)) -> (u32, u32) {
        let buffer_size = self.buffer_dimensions();
        let buffer_size = (
            buffer_size.0 as f32 * hidpi_scale,
            buffer_size.1 as f32 * hidpi_scale,
        );
        let max_width = screen_size.0 - 2. * DEFAULT_MARGIN;
        if let Some(dimensions) = self
            .size
            .as_ref()
            .map(|image_size| self.dimensions_from_image_size(image_size))
        {
            let target_dimensions = (
                (dimensions.0 as f32 * hidpi_scale) as u32,
                (dimensions.1 as f32 * hidpi_scale) as u32,
            );
            if target_dimensions.0 > max_width as u32 {
                (
                    max_width as u32,
                    ((max_width / buffer_size.0 as f32) * buffer_size.1 as f32) as u32,
                )
            } else {
                target_dimensions
            }
        } else if buffer_size.0 * hidpi_scale > max_width {
            (
                max_width as u32,
                ((max_width / buffer_size.0 as f32) * buffer_size.1 as f32) as u32,
            )
        } else {
            (buffer_size.0 as u32, buffer_size.1 as u32)
        }
    }

    pub fn get_image_data(&mut self) -> MutexGuard<Option<RgbaImage>> {
        self.image.lock().unwrap()
    }

    pub fn size(&self, hidpi_scale: f32, screen_size: (f32, f32)) -> (f32, f32) {
        let dimensions = self.dimensions(hidpi_scale, screen_size);
        (dimensions.0 as f32, dimensions.1 as f32)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct ImageVertex {
    pub pos: [f32; 3],
    pub tex_coords: [f32; 2],
}
pub struct ImageRenderer {
    pub render_pipeline: wgpu::RenderPipeline,
    pub index_buf: wgpu::Buffer,
    pub bindgroup_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
}

pub fn point(
    x: f32,
    y: f32,
    position: (f32, f32),
    size: (f32, f32),
    screen: (f32, f32),
) -> [f32; 3] {
    let scale_x = size.0 / screen.0;
    let scale_y = size.1 / screen.1;
    let shift_x = (position.0 / screen.0) * 2.;
    let shift_y = (position.1 / screen.1) * 2.;
    let new_x = (x * scale_x) - (1. - scale_x) + shift_x;
    let new_y = (y * scale_y) + (1. - scale_y) - shift_y;
    [new_x, new_y, 0.]
}

impl ImageRenderer {
    pub fn new(device: &Device, format: &TextureFormat) -> Self {
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
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
                label: Some("texture_bind_group_layout"),
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let vertex_buffers = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ImageVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
        }];

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("shaders/image.wgsl"))),
        });
        let image_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &vertex_buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: *format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            operation: wgpu::BlendOperation::Add,
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        },
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        const INDICES: &[u16] = &[0, 1, 2, 2, 3, 4];
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Index Buffer"),
            contents: bytemuck::cast_slice(INDICES),
            usage: wgpu::BufferUsages::INDEX,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self {
            render_pipeline: image_pipeline,
            index_buf,
            bindgroup_layout: texture_bind_group_layout,
            sampler,
        }
    }

    pub fn vertex_buf(
        device: &Device,
        pos: (f32, f32),
        size: (f32, f32),
        screen_size: (f32, f32),
    ) -> wgpu::Buffer {
        let vertices: &[ImageVertex] = &[
            ImageVertex {
                pos: point(-1.0, 1.0, pos, size, screen_size),
                tex_coords: [0.0, 0.0],
            },
            ImageVertex {
                pos: point(-1.0, -1.0, pos, size, screen_size),
                tex_coords: [0.0, 1.0],
            },
            ImageVertex {
                pos: point(1.0, -1.0, pos, size, screen_size),
                tex_coords: [1.0, 1.0],
            },
            ImageVertex {
                pos: point(1.0, 1.0, pos, size, screen_size),
                tex_coords: [1.0, 0.0],
            },
        ];
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        })
    }
}