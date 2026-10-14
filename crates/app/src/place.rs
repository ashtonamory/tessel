//! Getting image bytes into the editor: the file dialog and the clipboard.

use std::path::Path;

/// Encoded image bytes with the file extension to store them under and a
/// name for the node.
pub struct Picked {
    pub bytes: Vec<u8>,
    pub ext: String,
    pub name: String,
}

/// Read an image file picked by the user.
pub fn read_file(path: &Path) -> std::io::Result<Picked> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Image".into());
    Ok(Picked {
        bytes: std::fs::read(path)?,
        ext,
        name,
    })
}

/// Ask for an image file with the system dialog.
pub fn pick_file() -> Option<std::io::Result<Picked>> {
    let path = rfd::FileDialog::new()
        .set_title("Place image")
        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
        .pick_file()?;
    Some(read_file(&path))
}

/// Encode raw RGBA pixels as PNG.
pub fn encode_png(width: u32, height: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(width, height, rgba)?;
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// The image on the clipboard, as PNG, if there is one.
pub fn clipboard_image() -> Option<Picked> {
    let data = arboard::Clipboard::new().ok()?.get_image().ok()?;
    let bytes = encode_png(
        data.width as u32,
        data.height as u32,
        data.bytes.into_owned(),
    )?;
    Some(Picked {
        bytes,
        ext: "png".into(),
        name: "Pasted image".into(),
    })
}
