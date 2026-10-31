//! A GPU device with no window. Rendering runs on vello's compute shaders
//! through wgpu; a computer with no usable GPU falls back to a software
//! Vulkan driver (Mesa's lavapipe), which is slower but draws the same.

use render::{wgpu, SceneRenderer};

/// `TESSEL_GPU=software` skips the hardware adapter.
pub const GPU_ENV: &str = "TESSEL_GPU";

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error(
        "no GPU can render here ({0}). Install a Vulkan driver, or Mesa's lavapipe for \
         software rendering (vulkan-swrast on Arch, mesa-vulkan-drivers on Debian and Ubuntu)"
    )]
    NoAdapter(String),
}

/// The device, its queue and the renderer, made once and reused.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub renderer: SceneRenderer,
    /// The adapter's name, e.g. "AMD Radeon 780M" or "llvmpipe".
    pub adapter: String,
    /// Whether this is a software (CPU) adapter.
    pub software: bool,
}

impl Gpu {
    /// The hardware adapter if one works, else the software fallback.
    pub fn open() -> Result<Gpu, GpuError> {
        let forced = std::env::var(GPU_ENV).is_ok_and(|v| v.eq_ignore_ascii_case("software"));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut why = Vec::new();
        let tries: &[bool] = if forced { &[true] } else { &[false, true] };
        for &fallback in tries {
            match try_adapter(&instance, fallback) {
                Ok(gpu) => {
                    if fallback && !forced {
                        log::warn!(
                            "no hardware GPU renders here; using {} (slower)",
                            gpu.adapter
                        );
                    }
                    return Ok(gpu);
                }
                Err(e) => why.push(format!(
                    "{}: {e}",
                    if fallback { "software" } else { "hardware" }
                )),
            }
        }
        Err(GpuError::NoAdapter(why.join("; ")))
    }
}

fn try_adapter(instance: &wgpu::Instance, fallback: bool) -> Result<Gpu, String> {
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        force_fallback_adapter: fallback,
        ..Default::default()
    }))
    .map_err(|e| e.to_string())?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| e.to_string())?;
    let renderer = SceneRenderer::new(&device).map_err(|e| e.to_string())?;
    Ok(Gpu {
        device,
        queue,
        renderer,
        software: info.device_type == wgpu::DeviceType::Cpu,
        adapter: info.name,
    })
}
