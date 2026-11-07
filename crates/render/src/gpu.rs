//! Rendering scenes with vello on a caller-provided wgpu device (§7.2): the
//! app passes eframe's device so canvas and UI share one GPU context.

use std::num::NonZeroUsize;

use peniko::Color;
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("vello: {0}")]
    Vello(#[from] vello::Error),
    #[error("reading back the render failed: {0}")]
    Readback(String),
    #[error("this GPU device cannot run vello: {0}")]
    Unsupported(String),
    #[error("wgpu: {0}")]
    Wgpu(String),
    #[error(
        "the graphics driver ran vello's shaders but drew nothing correct \
         ({0}); its shader compiler likely does not support them"
    )]
    SelfTest(String),
}

/// The vello target format.
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A vello renderer, created once per device.
pub struct SceneRenderer {
    renderer: Renderer,
}

impl SceneRenderer {
    /// Fails, rather than panicking, on devices without the compute
    /// support vello needs (e.g. a GL device created with WebGL2 limits).
    pub fn new(device: &wgpu::Device) -> Result<SceneRenderer, RenderError> {
        let limits = device.limits();
        if limits.max_storage_buffers_per_shader_stage == 0
            || limits.max_compute_workgroups_per_dimension == 0
        {
            return Err(RenderError::Unsupported(
                "the device has no compute shader storage buffers".into(),
            ));
        }
        // Error scopes are per thread, so build the shaders on this one.
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let renderer = Renderer::new(
            device,
            RendererOptions {
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: NonZeroUsize::new(1),
                ..Default::default()
            },
        );
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(RenderError::Unsupported(e.to_string()));
        }
        Ok(SceneRenderer {
            renderer: renderer?,
        })
    }

    /// Draw a tiny known picture and read it back (§7.2). Some drivers
    /// accept vello's shaders without an error yet fail to compile them
    /// (nouveau on Kepler GPUs logs "shader translation failed"), which
    /// leaves every render blank. Call this once after [`SceneRenderer::new`]
    /// so such a device is reported instead of drawing a white canvas.
    pub fn self_test(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(), RenderError> {
        const SIZE: u32 = 16;
        let mut scene = Scene::new();
        scene.fill(
            peniko::Fill::NonZero,
            vello::kurbo::Affine::IDENTITY,
            Color::from_rgb8(255, 0, 0),
            None,
            &vello::kurbo::Rect::new(0.0, 0.0, f64::from(SIZE / 2), f64::from(SIZE)),
        );
        let px = self.render_to_rgba(
            device,
            queue,
            &scene,
            SIZE,
            SIZE,
            Color::from_rgb8(0, 0, 255),
        )?;
        let at = |x: u32, y: u32| {
            let i = ((y * SIZE + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        let close =
            |got: [u8; 4], want: [u8; 4]| got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 8);
        let (left, right) = (at(3, 8), at(12, 8));
        if close(left, [255, 0, 0, 255]) && close(right, [0, 0, 255, 255]) {
            Ok(())
        } else {
            Err(RenderError::SelfTest(format!(
                "expected red and blue, got {left:?} and {right:?}"
            )))
        }
    }

    /// Render `scene` into `target`.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        target: &CanvasTarget,
        background: Color,
    ) -> Result<(), RenderError> {
        let _span = tracing::trace_span!("render").entered();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.renderer.render_to_texture(
            device,
            queue,
            scene,
            &target.view,
            &RenderParams {
                base_color: background,
                width: target.width,
                height: target.height,
                antialiasing_method: AaConfig::Area,
            },
        );
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(RenderError::Wgpu(e.to_string()));
        }
        result?;
        Ok(())
    }

    /// Render `scene` offscreen and read it back as tightly packed RGBA8
    /// rows (§7.6). Blocks until the GPU is done.
    pub fn render_to_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        width: u32,
        height: u32,
        background: Color,
    ) -> Result<Vec<u8>, RenderError> {
        let target = CanvasTarget::new(device, width, height);
        self.render(device, queue, scene, &target, background)?;

        target.read_rgba(device, queue)
    }
}

/// A texture vello renders into and the UI samples from.
pub struct CanvasTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

impl CanvasTarget {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> CanvasTarget {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        CanvasTarget {
            texture,
            view,
            width: width.max(1),
            height: height.max(1),
        }
    }

    /// Read the texture back as tightly packed RGBA8 rows. Blocks until the
    /// GPU is done.
    pub fn read_rgba(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Vec<u8>, RenderError> {
        let padded = (self.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: padded as u64 * self.height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("readback"),
        });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: None,
                },
            },
            self.texture.size(),
        );
        queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| RenderError::Readback(e.to_string()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        for row in data.chunks(padded as usize) {
            out.extend_from_slice(&row[..(self.width * 4) as usize]);
        }
        drop(data);
        buffer.unmap();
        Ok(out)
    }
}
