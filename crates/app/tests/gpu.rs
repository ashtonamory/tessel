//! The canvas on a real wgpu device (§7.2): vello renders into a texture
//! on the same device egui uses and the texture is registered with egui's
//! renderer. Skips when no adapter is found.

mod common;

use common::Harness;
use egui::{Key, Modifiers};

/// GPU tests run one at a time: creating GL contexts concurrently crashes
/// some drivers (Mesa llvmpipe).
fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn render_state() -> Option<egui_wgpu::RenderState> {
    let config = app::app::wgpu_configuration();
    let egui_wgpu::WgpuSetup::CreateNew(setup) = &config.wgpu_setup else {
        unreachable!("the app creates its own device");
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: setup.instance_descriptor.backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    match pollster::block_on(egui_wgpu::RenderState::create(
        &config,
        &instance,
        None,
        egui_wgpu::RendererOptions::default(),
    )) {
        Ok(rs) => {
            eprintln!("GPU app test on {:?}", rs.adapter.get_info());
            Some(rs)
        }
        Err(e) => {
            eprintln!("skipping GPU app test: {e}");
            None
        }
    }
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

#[test]
fn canvas_renders_the_document_into_a_registered_texture() {
    let _gpu = gpu_lock();
    let Some(rs) = render_state() else { return };
    let mut frame = eframe::Frame::_new_kittest();
    frame.wgpu_render_state = Some(rs.clone());
    let mut h = Harness::with_frame(frame);
    if let Some(e) = h.app.canvas().gpu_error() {
        eprintln!("skipping GPU app test: vello cannot run here ({e})");
        return;
    }

    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(10.0, 10.0), h.at(110.0, 60.0));
    h.step(vec![]);

    let (target, texture_id) = h.app.canvas().target().expect("canvas target");
    assert!(
        rs.renderer.read().texture(&texture_id).is_some(),
        "the canvas texture is registered with egui"
    );
    let rect = h.app.canvas().rect;
    assert_eq!(target.width, rect.width().round() as u32);
    assert_eq!(target.height, rect.height().round() as u32);

    let rgba = target.read_rgba(&rs.device, &rs.queue).expect("read back");
    // Pixel coordinates inside the texture (pixels_per_point is 1).
    let inside = h.at(60.0, 35.0) - rect.min;
    let outside = h.at(200.0, 200.0) - rect.min;
    let (w, x, y) = (target.width, inside.x as u32, inside.y as u32);
    assert_eq!(pixel(&rgba, w, x, y), [0xd9, 0xd9, 0xd9, 0xff], "rect fill");
    assert_eq!(
        pixel(&rgba, w, outside.x as u32, outside.y as u32),
        [0xf5, 0xf5, 0xf5, 0xff],
        "page background"
    );
}

#[test]
fn renderer_init_fails_cleanly_on_devices_without_compute() {
    let _gpu = gpu_lock();
    // egui's default GL device (WebGL2 limits) has no compute storage
    // buffers; creating the canvas renderer there must be an error, not a
    // panic.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        eprintln!("skipping: no adapter");
        return;
    };
    let desc = wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
        ..Default::default()
    };
    let Ok((device, _queue)) = pollster::block_on(adapter.request_device(&desc)) else {
        eprintln!("skipping: no device");
        return;
    };
    assert!(render::SceneRenderer::new(&device).is_err());
}

#[test]
fn agent_screenshots_and_png_exports_render_on_the_apps_device() {
    use common::mcp::Session;
    use serde_json::json;

    let _gpu = gpu_lock();
    let Some(rs) = render_state() else { return };
    let mut frame = eframe::Frame::_new_kittest();
    frame.wgpu_render_state = Some(rs);
    let mut h = Harness::with_frame(frame);
    if let Some(e) = h.app.canvas().gpu_error() {
        eprintln!("skipping GPU app test: vello cannot run here ({e})");
        return;
    }
    let dir = std::env::temp_dir().join(format!("tessel-app-shot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    h.app.editor_mut().path = Some(dir.join("a.tessel.json"));

    let mut a = Session::connect(&mut h, "claude-code");
    let page = h.ed().page().id.to_string();
    let args = json!({ "parent_id": page, "node": {
        "name": "Card", "kind": "frame", "x": 0, "y": 0, "width": 200, "height": 100,
        "fills": [{ "type": "solid", "color": "#ff0000" }] } });
    let id = a.ok(&mut h, "create_node", args)["id"].clone();
    // Select it, so selection chrome would show if it leaked in.
    a.ok(&mut h, "set_selection", json!({ "ids": [id] }));

    let r = a.call(
        &mut h,
        "screenshot",
        json!({ "id": id, "scale": 2, "max_size": 300 }),
    );
    assert!(!r.is_error, "{}", r.json);
    assert_eq!(r.json, json!({ "width": 300, "height": 150, "scale": 1.5 }));
    let data = r.raw["content"][0]["data"].as_str().expect("image content");
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (300, 150));
    assert_eq!(img.get_pixel(150, 75).0, [0xff, 0, 0, 0xff]);
    assert_eq!(
        img.get_pixel(0, 0).0,
        [0xff, 0, 0, 0xff],
        "no selection chrome"
    );

    let out = dir.join("card@2x.png");
    let w = a.ok(
        &mut h,
        "export_file",
        json!({ "id": id, "format": "png", "path": out.to_str().unwrap(), "scale": 2 }),
    );
    let img = image::open(&out).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (400, 200));
    assert_eq!(
        w["bytes"].as_u64(),
        Some(std::fs::metadata(&out).unwrap().len())
    );
    let _ = std::fs::remove_dir_all(dir);
}
