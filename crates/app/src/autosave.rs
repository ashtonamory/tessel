//! Autosave and crash recovery (§4.5): unsaved work is written every 30 s
//! to `$XDG_STATE_HOME/tessel/recovery/` (or `~/.local/state/...`),
//! offered back on the next launch, and deleted on a clean save or quit.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::editor::Editor;

pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);

/// The recovery directory for the given `$XDG_STATE_HOME` and `$HOME`.
/// A relative or empty `XDG_STATE_HOME` is ignored, as the XDG spec says.
pub fn recovery_dir_from(xdg_state: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = xdg_state
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            home.filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".local/state"))
        })?;
    Some(base.join("tessel").join("recovery"))
}

/// The recovery directory for this user, from the environment.
pub fn default_recovery_dir() -> Option<PathBuf> {
    recovery_dir_from(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
}

/// The recovery file for `source` (`None` is an unsaved, untitled document).
/// The name carries a hash of the full path, so two files called `a.tessel.json`
/// in different folders do not share a recovery file.
pub fn recovery_path(dir: &Path, source: Option<&Path>) -> PathBuf {
    let Some(source) = source else {
        return dir.join("untitled.tessel.json");
    };
    let stem: String = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    // FNV-1a: stable across runs and Rust versions, unlike `DefaultHasher`.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in source.to_string_lossy().bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    dir.join(format!("{stem}-{h:016x}.tessel.json"))
}

/// Write the document to its recovery file, atomically, with the images
/// that exist only in memory in the `.assets` directory beside it. The live
/// document keeps reading its assets from where it did.
pub fn write_recovery(dir: &Path, source: Option<&Path>, ed: &Editor) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = recovery_path(dir, source);
    // The document first: a failed image write must not lose the edits.
    doc::save_file(&path, ed.document())?;
    if let Err(e) = ed
        .assets
        .write_to(&doc::assets::assets_dir_for(&path), ed.document())
    {
        tracing::warn!("autosave: cannot write images: {e}");
    }
    Ok(path)
}

/// A recovery file left by an earlier run, if it is newer than the file it
/// recovers (or that file is gone or never existed).
pub fn find_recovery(dir: &Path, source: Option<&Path>) -> Option<PathBuf> {
    let path = recovery_path(dir, source);
    let recovered = std::fs::metadata(&path).ok()?.modified().ok()?;
    let saved: Option<SystemTime> = source
        .and_then(|s| std::fs::metadata(s).ok())
        .and_then(|m| m.modified().ok());
    match saved {
        Some(saved) if saved >= recovered => None,
        _ => Some(path),
    }
}

/// Delete the recovery file of `source`, if any.
pub fn remove_recovery(dir: &Path, source: Option<&Path>) {
    let path = recovery_path(dir, source);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != io::ErrorKind::NotFound {
            tracing::warn!("cannot remove {}: {e}", path.display());
        }
    }
    let assets = doc::assets::assets_dir_for(&path);
    if let Err(e) = std::fs::remove_dir_all(&assets) {
        if e.kind() != io::ErrorKind::NotFound {
            tracing::warn!("cannot remove {}: {e}", assets.display());
        }
    }
}

/// Open the recovery file as the document of `source`, unsaved.
pub fn restore(recovery: &Path, source: Option<&Path>) -> anyhow::Result<Editor> {
    let mut ed = Editor::open(recovery)?;
    ed.path = source.map(Path::to_path_buf);
    ed.assets.set_dir(source.map(doc::assets::assets_dir_for));
    // Image bytes that never reached the source's assets directory.
    ed.assets
        .load_dir(&doc::assets::assets_dir_for(recovery), ed.document());
    ed.mark_dirty();
    Ok(ed)
}

/// Decides when to write the next recovery file.
pub struct Autosaver {
    pub dir: Option<PathBuf>,
    last: Instant,
    written_revision: Option<u64>,
}

impl Autosaver {
    pub fn new(dir: Option<PathBuf>, now: Instant) -> Autosaver {
        Autosaver {
            dir,
            last: now,
            written_revision: None,
        }
    }

    /// Write a recovery file if the document has had unsaved edits for 30 s
    /// since the last write. Returns the error text on failure.
    pub fn tick(&mut self, ed: &Editor, now: Instant) -> Option<String> {
        let dir = self.dir.as_ref()?;
        if !ed.is_dirty() {
            // The 30 s start when the first unsaved edit does.
            self.last = now;
            self.written_revision = None;
            return None;
        }
        if now.saturating_duration_since(self.last) < AUTOSAVE_INTERVAL
            || self.written_revision == Some(ed.revision)
        {
            return None;
        }
        self.last = now;
        match write_recovery(dir, ed.path.as_deref(), ed) {
            Ok(_) => {
                self.written_revision = Some(ed.revision);
                None
            }
            Err(e) => {
                tracing::warn!("autosave: {e}");
                Some(format!("Autosave failed: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(format!("autosave-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn recovery_dir_follows_xdg_then_home() {
        let d = |x: Option<&str>, h: Option<&str>| {
            recovery_dir_from(x.map(Into::into), h.map(Into::into))
        };
        assert_eq!(
            d(Some("/state"), Some("/home/a")),
            Some(PathBuf::from("/state/tessel/recovery"))
        );
        assert_eq!(
            d(None, Some("/home/a")),
            Some(PathBuf::from("/home/a/.local/state/tessel/recovery"))
        );
        // Relative or empty XDG_STATE_HOME is ignored.
        assert_eq!(
            d(Some("rel"), Some("/home/a")),
            Some(PathBuf::from("/home/a/.local/state/tessel/recovery"))
        );
        assert_eq!(d(Some(""), None), None);
    }

    #[test]
    fn recovery_paths_are_stable_and_distinct_per_source() {
        let dir = Path::new("/r");
        let a = recovery_path(dir, Some(Path::new("/p/one/a.tessel.json")));
        let b = recovery_path(dir, Some(Path::new("/p/two/a.tessel.json")));
        assert_ne!(a, b);
        assert_eq!(
            a,
            recovery_path(dir, Some(Path::new("/p/one/a.tessel.json")))
        );
        assert!(a.starts_with(dir) && a.to_string_lossy().ends_with(".tessel.json"));
        assert_eq!(
            recovery_path(dir, None),
            Path::new("/r/untitled.tessel.json")
        );
    }

    #[test]
    fn write_is_atomic_and_round_trips() {
        let dir = temp("write").join("nested/recovery");
        let ed = Editor::new();
        let src = Path::new("/x/y.tessel.json");
        let p = write_recovery(&dir, Some(src), &ed).unwrap();
        assert_eq!(p, recovery_path(&dir, Some(src)));
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "no .tmp file may remain: {leftovers:?}");
        assert_eq!(doc::load_file(&p).unwrap().doc, ed.doc);
        remove_recovery(&dir, Some(src));
        assert!(!p.exists());
        remove_recovery(&dir, Some(src)); // already gone: fine
    }

    #[test]
    fn recovery_is_offered_only_when_newer_than_the_source() {
        let root = temp("detect");
        let dir = root.join("rec");
        let src = root.join("a.tessel.json");
        let ed = Editor::new();
        assert_eq!(find_recovery(&dir, Some(&src)), None);

        // Source gone: offer.
        let rec = write_recovery(&dir, Some(&src), &ed).unwrap();
        assert_eq!(find_recovery(&dir, Some(&src)), Some(rec.clone()));

        // Source saved after the recovery file: stale, do not offer.
        let later = SystemTime::now() + Duration::from_secs(60);
        std::fs::write(&src, "x").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(find_recovery(&dir, Some(&src)), None);

        // Recovery newer than the source: offer.
        std::fs::File::options()
            .write(true)
            .open(&rec)
            .unwrap()
            .set_modified(later + Duration::from_secs(60))
            .unwrap();
        assert_eq!(find_recovery(&dir, Some(&src)), Some(rec));

        // Untitled documents have no source to compare with.
        assert_eq!(find_recovery(&dir, None), None);
        let u = write_recovery(&dir, None, &ed).unwrap();
        assert_eq!(find_recovery(&dir, None), Some(u));
    }

    #[test]
    fn restore_gives_an_unsaved_document_at_the_source_path() {
        let root = temp("restore");
        let src = root.join("a.tessel.json");
        let rec = write_recovery(&root.join("rec"), Some(&src), &Editor::new()).unwrap();
        let ed = restore(&rec, Some(&src)).unwrap();
        assert_eq!(ed.path.as_deref(), Some(src.as_path()));
        assert!(ed.is_dirty());
    }

    #[test]
    fn autosaver_writes_every_30s_only_while_dirty() {
        let dir = temp("tick").join("rec");
        let clock = crate::activity::Clock::manual();
        let mut ed = Editor::new();
        ed.clock = clock.clone();
        let mut a = Autosaver::new(Some(dir.clone()), clock.now());
        let file = recovery_path(&dir, None);

        // Clean: never writes, however long we wait.
        clock.advance(Duration::from_secs(120));
        assert_eq!(a.tick(&ed, clock.now()), None);
        assert!(!file.exists());

        // Dirty: the clock starts at the first tick that sees the edit.
        ed.mark_dirty();
        a.tick(&ed, clock.now());
        clock.advance(Duration::from_secs(29));
        a.tick(&ed, clock.now());
        assert!(!file.exists());
        clock.advance(Duration::from_secs(1));
        a.tick(&ed, clock.now());
        assert!(file.exists());

        // Nothing new to write: stays quiet; a new edit writes again.
        std::fs::remove_file(&file).unwrap();
        clock.advance(Duration::from_secs(31));
        a.tick(&ed, clock.now());
        assert!(!file.exists());
        ed.mark_dirty();
        clock.advance(Duration::from_secs(31));
        a.tick(&ed, clock.now());
        assert!(file.exists());
    }
}
