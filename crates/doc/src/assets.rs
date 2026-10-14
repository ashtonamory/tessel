//! Image assets (§3.8): content-addressed bytes kept next to the document in
//! `<name>.assets/<sha256-prefix>.<ext>`.
//!
//! An [`Assets`] is a cheap, shareable handle. Bytes added to it live in
//! memory until a save writes them out; bytes that are not in memory are read
//! lazily from the assets directory of the file the document was loaded from.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use sha2::{Digest, Sha256};

use crate::model::{AssetRef, Document, NodeKind, Paint};

/// Largest asset file read from disk. An assets folder next to a downloaded
/// design is untrusted.
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

/// Hex characters of the SHA-256 digest used in an asset's file name.
const HASH_LEN: usize = 12;

#[derive(Default)]
struct Inner {
    mem: HashMap<AssetRef, Arc<Vec<u8>>>,
    dir: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct Assets {
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for Assets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.lock();
        f.debug_struct("Assets")
            .field("in_memory", &inner.mem.len())
            .field("dir", &inner.dir)
            .finish()
    }
}

/// `landing.tessel.json` (or the legacy `landing.design.json`) -> `landing.assets` next to it.
pub fn assets_dir_for(design_file: &Path) -> PathBuf {
    let name = design_file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name
        .strip_suffix(".tessel.json")
        .or_else(|| name.strip_suffix(".design.json"))
        .or_else(|| name.strip_suffix(".json"))
        .unwrap_or(&name);
    design_file.with_file_name(format!("{stem}.assets"))
}

/// The asset name for `bytes` with file extension `ext` (no dot).
pub fn asset_ref_for(bytes: &[u8], ext: &str) -> AssetRef {
    let digest = Sha256::digest(bytes);
    let mut name = String::new();
    for b in digest.iter().take(HASH_LEN / 2) {
        name.push_str(&format!("{b:02x}"));
    }
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    if !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        name.push('.');
        name.push_str(&ext);
    }
    AssetRef(name)
}

/// Is this a name we generate (so pruning may delete it)?
fn is_managed_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("");
    stem.len() == HASH_LEN && stem.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Read `path` only if it is a regular file (not a symlink, FIFO or
/// directory) of at most [`MAX_ASSET_BYTES`]; otherwise warn and return `None`.
fn read_regular(path: &Path) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        tracing::warn!("asset {} is not a regular file; skipped", path.display());
        return None;
    }
    if meta.len() > MAX_ASSET_BYTES {
        tracing::warn!("asset {} is too large; skipped", path.display());
        return None;
    }
    use std::io::Read;
    let mut out = Vec::new();
    // Bounded even if the file grows after the check.
    fs::File::open(path)
        .ok()?
        .take(MAX_ASSET_BYTES + 1)
        .read_to_end(&mut out)
        .ok()?;
    if out.len() as u64 > MAX_ASSET_BYTES {
        tracing::warn!("asset {} is too large; skipped", path.display());
        return None;
    }
    Some(out)
}

/// Every asset a document references, by node image or image paint.
pub fn referenced(doc: &Document) -> BTreeSet<AssetRef> {
    fn paints(out: &mut BTreeSet<AssetRef>, ps: &[Paint]) {
        for p in ps {
            if let Paint::Image(i) = p {
                out.insert(i.asset.clone());
            }
        }
    }
    let mut out = BTreeSet::new();
    for page in &doc.pages {
        for top in &page.children {
            top.walk(&mut |n| match &n.kind {
                NodeKind::Image(i) => {
                    out.insert(i.asset.clone());
                }
                NodeKind::Frame(f) => {
                    paints(&mut out, &f.shape.fills);
                    paints(&mut out, &f.shape.strokes);
                }
                NodeKind::Rect(r) => {
                    paints(&mut out, &r.shape.fills);
                    paints(&mut out, &r.shape.strokes);
                }
                NodeKind::Ellipse(e) => {
                    paints(&mut out, &e.shape.fills);
                    paints(&mut out, &e.shape.strokes);
                }
                NodeKind::Vector(v) => {
                    paints(&mut out, &v.shape.fills);
                    paints(&mut out, &v.shape.strokes);
                }
                NodeKind::Text(t) => paints(&mut out, &t.fills),
                _ => {}
            });
        }
    }
    out
}

impl Assets {
    pub fn new() -> Assets {
        Assets::default()
    }

    /// Do both handles share one store?
    pub fn same(&self, other: &Assets) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Store `bytes`; the same content always yields the same name and is
    /// kept once.
    pub fn add(&self, bytes: Vec<u8>, ext: &str) -> AssetRef {
        let r = asset_ref_for(&bytes, ext);
        self.lock()
            .mem
            .entry(r.clone())
            .or_insert_with(|| Arc::new(bytes));
        r
    }

    /// Read lazily from `dir` (the assets directory of the loaded file).
    pub fn set_dir(&self, dir: Option<PathBuf>) {
        self.lock().dir = dir;
    }

    /// The bytes of an asset, from memory or the assets directory. `None`
    /// if missing or if the name tries to leave the directory.
    pub fn bytes(&self, r: &AssetRef) -> Option<Arc<Vec<u8>>> {
        let mut inner = self.lock();
        if let Some(b) = inner.mem.get(r) {
            return Some(b.clone());
        }
        if r.0.contains(['/', '\\']) || r.0.starts_with('.') || r.0.is_empty() {
            return None;
        }
        let bytes = Arc::new(read_regular(&inner.dir.as_ref()?.join(&r.0))?);
        inner.mem.insert(r.clone(), bytes.clone());
        Some(bytes)
    }

    /// Write the referenced assets that `dir` does not have yet. Creates
    /// `dir` only when there is something to write. Neither prunes nor
    /// changes where assets are read from.
    pub fn write_to(&self, dir: &Path, doc: &Document) -> std::io::Result<()> {
        let used = referenced(doc);
        // Read everything first: `dir` may be the old directory being replaced.
        let mut to_write = Vec::new();
        for r in &used {
            if dir.join(&r.0).is_file() {
                continue;
            }
            if let Some(b) = self.bytes(r) {
                to_write.push((r.clone(), b));
            } else {
                tracing_warn(&r.0);
            }
        }
        if !to_write.is_empty() {
            fs::create_dir_all(dir)?;
        }
        for (r, b) in to_write {
            let final_path = dir.join(&r.0);
            let tmp = dir.join(format!("{}.tmp", r.0));
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&b)?;
            f.sync_all()?;
            fs::rename(tmp, final_path)?;
        }
        Ok(())
    }

    /// Read the assets `doc` references from `dir` into memory, so a later
    /// save writes them beside the document. Other files in `dir`, and
    /// anything that is not a plain, reasonably sized file, are ignored.
    pub fn load_dir(&self, dir: &Path, doc: &Document) {
        for r in referenced(doc) {
            if r.0.contains(['/', '\\']) || r.0.starts_with('.') || r.0.is_empty() {
                continue;
            }
            if self.lock().mem.contains_key(&r) {
                continue;
            }
            if let Some(bytes) = read_regular(&dir.join(&r.0)) {
                self.lock().mem.insert(r, Arc::new(bytes));
            }
        }
    }

    /// Where assets are read from, if anywhere.
    pub fn dir(&self) -> Option<PathBuf> {
        self.lock().dir.clone()
    }

    /// Write the referenced assets to `dir` and prune managed files nobody
    /// references (§3.8). Creates `dir` only when there is something to
    /// write; removes it when it ends up empty. Afterwards `dir` is where
    /// assets are read from.
    pub fn save(&self, dir: &Path, doc: &Document) -> std::io::Result<()> {
        let used = referenced(doc);
        self.write_to(dir, doc)?;
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let r = AssetRef(name);
                if is_managed_name(&r.0) && !used.contains(&r) {
                    // Keep the bytes so an undo can still render and re-save it.
                    let _ = self.bytes(&r);
                    let _ = fs::remove_file(e.path());
                }
            }
            // Only succeeds when empty.
            let _ = fs::remove_dir(dir);
        }
        self.set_dir(Some(dir.to_path_buf()));
        Ok(())
    }
}

fn tracing_warn(name: &str) {
    eprintln!("asset {name} is referenced but its bytes are missing; not saved");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_dir_maps_current_and_legacy_extensions() {
        for name in ["a.tessel.json", "a.design.json"] {
            assert_eq!(
                assets_dir_for(Path::new("/p").join(name).as_path()),
                Path::new("/p/a.assets")
            );
        }
    }
}
