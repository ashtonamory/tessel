//! Raster images: a decode cache keyed by asset name, and drawing an image
//! into a shape with one of the `ImageFit` modes (§3.4, §7.4).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use doc::{AssetRef, Assets, ImageFit};
use kurbo::{Affine, BezPath, Point, Rect, Stroke};
use peniko::{
    Blob, Color, Extend, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality,
};
use vello::Scene;

/// Longest side kept when decoding; bigger images are downscaled so they fit
/// GPU texture limits.
const MAX_SIDE: u32 = 4096;

/// Decoded images, each decoded at most once. A failed decode is cached too,
/// so a broken file is not retried every frame. Cheap to clone; clones share
/// the cache.
#[derive(Clone, Default)]
pub struct ImageCache {
    assets: Assets,
    decoded: Arc<Mutex<HashMap<AssetRef, Option<ImageData>>>>,
}

impl ImageCache {
    pub fn new(assets: Assets) -> ImageCache {
        ImageCache {
            assets,
            decoded: Default::default(),
        }
    }

    /// The store this cache decodes from.
    pub fn assets(&self) -> &Assets {
        &self.assets
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<AssetRef, Option<ImageData>>> {
        self.decoded.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The decoded image, or `None` if the asset is missing or not an image.
    pub fn get(&self, asset: &AssetRef) -> Option<ImageData> {
        if let Some(hit) = self.lock().get(asset) {
            return hit.clone();
        }
        let decoded = self.decode(asset);
        self.lock().insert(asset.clone(), decoded.clone());
        decoded
    }

    fn decode(&self, asset: &AssetRef) -> Option<ImageData> {
        let Some(bytes) = self.assets.bytes(asset) else {
            tracing::warn!("image asset {} is missing", asset.0);
            return None;
        };
        match decode(&bytes) {
            Ok(img) => Some(img),
            Err(e) => {
                tracing::warn!("image asset {} cannot be decoded: {e}", asset.0);
                None
            }
        }
    }

    /// Forget everything, e.g. after the document was reloaded from disk.
    pub fn clear(&self) {
        self.lock().clear();
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }
}

/// Decode encoded image bytes to straight-alpha RGBA8.
pub fn decode(bytes: &[u8]) -> Result<ImageData, image::ImageError> {
    let mut img = image::load_from_memory(bytes)?;
    if img.width().max(img.height()) > MAX_SIDE {
        img = img.thumbnail(MAX_SIDE, MAX_SIDE);
    }
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(ImageData {
        data: Blob::new(Arc::new(rgba.into_raw())),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}

/// Natural size in pixels of encoded image bytes, without keeping the pixels.
pub fn natural_size(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Where the image goes inside a `w` x `h` box: the rect it covers and the
/// transform from image pixels to box coordinates.
fn placement(fit: ImageFit, w: f64, h: f64, iw: f64, ih: f64) -> (Rect, Affine, Extend) {
    let boxr = Rect::new(0.0, 0.0, w, h);
    match fit {
        ImageFit::Fill => (boxr, Affine::scale_non_uniform(w / iw, h / ih), Extend::Pad),
        ImageFit::Fit => {
            let s = (w / iw).min(h / ih);
            let (dw, dh) = (iw * s, ih * s);
            let (x, y) = ((w - dw) / 2.0, (h - dh) / 2.0);
            (
                Rect::new(x, y, x + dw, y + dh),
                Affine::translate((x, y)) * Affine::scale(s),
                Extend::Pad,
            )
        }
        ImageFit::Crop => {
            let s = (w / iw).max(h / ih);
            let (x, y) = ((w - iw * s) / 2.0, (h - ih * s) / 2.0);
            (
                boxr,
                Affine::translate((x, y)) * Affine::scale(s),
                Extend::Pad,
            )
        }
        ImageFit::Tile => (boxr, Affine::IDENTITY, Extend::Repeat),
    }
}

/// Fill `shape` (in the box's local coordinates, size `w` x `h`, under
/// `transform`) with the asset, clipped to the shape. A missing or
/// undecodable asset draws a placeholder instead.
#[allow(clippy::too_many_arguments)]
pub fn draw_image(
    scene: &mut Scene,
    transform: Affine,
    shape: &impl kurbo::Shape,
    size: (f64, f64),
    images: &ImageCache,
    asset: &AssetRef,
    fit: ImageFit,
) {
    let (w, h) = size;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    scene.push_clip_layer(Fill::NonZero, transform, shape);
    match images.get(asset) {
        Some(img) => {
            let (dest, tf, extend) = placement(fit, w, h, img.width as f64, img.height as f64);
            let brush = ImageBrush::new(img)
                .with_extend(extend)
                .with_quality(ImageQuality::Medium);
            scene.fill(Fill::NonZero, transform, &brush, Some(tf), &dest);
        }
        None => draw_missing(scene, transform, w, h),
    }
    scene.pop_layer();
}

/// The "missing image" placeholder: pale red box with a red cross.
fn draw_missing(scene: &mut Scene, transform: Affine, w: f64, h: f64) {
    let boxr = Rect::new(0.0, 0.0, w, h);
    scene.fill(
        Fill::NonZero,
        transform,
        Color::from_rgba8(0xfd, 0xe8, 0xe8, 0xff),
        None,
        &boxr,
    );
    let red = Color::from_rgba8(0xe0, 0x3b, 0x3b, 0xff);
    let mut cross = BezPath::new();
    cross.move_to(Point::ZERO);
    cross.line_to((w, h));
    cross.move_to((w, 0.0));
    cross.line_to((0.0, h));
    scene.stroke(&Stroke::new(2.0), transform, red, None, &cross);
    scene.stroke(&Stroke::new(2.0), transform, red, None, &boxr);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn decodes_once_and_reports_failures_without_panicking() {
        let assets = Assets::new();
        let good = assets.add(png(4, 2), "png");
        let bad = assets.add(b"not an image".to_vec(), "png");
        let cache = ImageCache::new(assets);
        let a = cache.get(&good).unwrap();
        let b = cache.get(&good).unwrap();
        assert_eq!((a.width, a.height), (4, 2));
        // The second get shares the first decode's pixels.
        assert_eq!(a.data.id(), b.data.id());
        assert!(cache.get(&bad).is_none());
        assert!(cache.get(&AssetRef("000000000000.png".into())).is_none());
        assert_eq!(cache.len(), 3);
        assert_eq!(natural_size(&png(4, 2)), Some((4, 2)));
        assert_eq!(natural_size(b"junk"), None);
    }

    #[test]
    fn placement_modes() {
        // 10x5 image into a 20x20 box.
        let (r, _, e) = placement(ImageFit::Fit, 20.0, 20.0, 10.0, 5.0);
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (0.0, 5.0, 20.0, 15.0));
        assert_eq!(e, Extend::Pad);
        let (r, tf, _) = placement(ImageFit::Crop, 20.0, 20.0, 10.0, 5.0);
        assert_eq!(r, Rect::new(0.0, 0.0, 20.0, 20.0));
        // Scale 4, centered horizontally: image is 40 wide, shifted -10.
        assert_eq!(tf.as_coeffs(), [4.0, 0.0, 0.0, 4.0, -10.0, 0.0]);
        let (_, tf, _) = placement(ImageFit::Fill, 20.0, 20.0, 10.0, 5.0);
        assert_eq!(tf.as_coeffs(), [2.0, 0.0, 0.0, 4.0, 0.0, 0.0]);
        let (_, _, e) = placement(ImageFit::Tile, 20.0, 20.0, 10.0, 5.0);
        assert_eq!(e, Extend::Repeat);
    }
}
