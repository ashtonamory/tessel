//! Reload on external change (§4.5): the open file's mtime is polled every
//! second; a change reloads the file silently, or asks first when there are
//! unsaved edits.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::editor::Editor;

pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// What a file looked like when we last read or wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    mtime: SystemTime,
    len: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Stamp> {
        let m = std::fs::metadata(path).ok()?;
        Some(Stamp {
            mtime: m.modified().ok()?,
            len: m.len(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    /// Changed on disk and nothing unsaved: reload silently.
    Reload,
    /// Changed on disk and there are unsaved edits: ask.
    Ask,
}

/// The reload rule. A file that is missing on disk is left alone (an
/// editor may be mid-way through replacing it).
pub fn decide(known: Option<Stamp>, on_disk: Option<Stamp>, dirty: bool) -> Decision {
    match on_disk {
        None => Decision::Nothing,
        Some(d) if Some(d) == known => Decision::Nothing,
        Some(_) if dirty => Decision::Ask,
        Some(_) => Decision::Reload,
    }
}

pub struct FileWatcher {
    path: Option<PathBuf>,
    known: Option<Stamp>,
    last_poll: Instant,
    /// A question is on screen; don't ask again until it is answered.
    asking: bool,
}

impl FileWatcher {
    pub fn new(now: Instant) -> FileWatcher {
        FileWatcher {
            path: None,
            known: None,
            last_poll: now,
            asking: false,
        }
    }

    /// Watch `path`, taking its current state as already seen. Also call
    /// this after our own save, so it does not look like an external edit.
    pub fn sync(&mut self, path: Option<&Path>) {
        self.path = path.map(Path::to_path_buf);
        self.known = path.and_then(Stamp::of);
        self.asking = false;
    }

    pub fn is_asking(&self) -> bool {
        self.asking
    }

    /// Stat the file if a second has passed since the last time.
    pub fn poll(&mut self, now: Instant, dirty: bool) -> Decision {
        if self.asking || now.saturating_duration_since(self.last_poll) < POLL_INTERVAL {
            return Decision::Nothing;
        }
        self.last_poll = now;
        let Some(path) = &self.path else {
            return Decision::Nothing;
        };
        let d = decide(self.known, Stamp::of(path), dirty);
        self.asking = d == Decision::Ask;
        d
    }
}

/// Re-read the editor's file and take it over, keeping the selection where
/// the IDs still exist.
pub fn reload(ed: &mut Editor) -> anyhow::Result<()> {
    let path = ed
        .path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("the document has no file"))?;
    let fresh = Editor::open(&path)?;
    ed.replace_with_reloaded(fresh);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(secs: u64, len: u64) -> Stamp {
        Stamp {
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
            len,
        }
    }

    #[test]
    fn reload_decision() {
        let a = Some(stamp(10, 5));
        let b = Some(stamp(11, 5));
        // Unchanged: nothing, dirty or not.
        assert_eq!(decide(a, a, false), Decision::Nothing);
        assert_eq!(decide(a, a, true), Decision::Nothing);
        // Changed, nothing unsaved: reload. Unsaved edits: ask.
        assert_eq!(decide(a, b, false), Decision::Reload);
        assert_eq!(decide(a, b, true), Decision::Ask);
        // Same mtime, different size still counts as a change.
        assert_eq!(decide(a, Some(stamp(10, 6)), false), Decision::Reload);
        // Deleted: leave it alone. A file that appeared: reload.
        assert_eq!(decide(a, None, false), Decision::Nothing);
        assert_eq!(decide(None, a, false), Decision::Reload);
    }

    #[test]
    fn polls_once_a_second_and_ignores_own_saves() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(format!("watch-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("w.tessel.json");
        std::fs::write(&path, "a").unwrap();

        let clock = crate::activity::Clock::manual();
        let mut w = FileWatcher::new(clock.now());
        w.sync(Some(&path));
        clock.advance(Duration::from_secs(1));
        assert_eq!(w.poll(clock.now(), false), Decision::Nothing);

        // An external change is seen at the next poll, not before.
        std::fs::write(&path, "bb").unwrap();
        clock.advance(Duration::from_millis(500));
        assert_eq!(w.poll(clock.now(), false), Decision::Nothing);
        clock.advance(Duration::from_millis(500));
        assert_eq!(w.poll(clock.now(), false), Decision::Reload);

        // Our own save, followed by sync, is not a change.
        std::fs::write(&path, "ccc").unwrap();
        w.sync(Some(&path));
        clock.advance(Duration::from_secs(1));
        assert_eq!(w.poll(clock.now(), true), Decision::Nothing);

        // With unsaved edits it asks once, then waits for the answer.
        std::fs::write(&path, "dddd").unwrap();
        clock.advance(Duration::from_secs(1));
        assert_eq!(w.poll(clock.now(), true), Decision::Ask);
        assert!(w.is_asking());
        clock.advance(Duration::from_secs(1));
        assert_eq!(w.poll(clock.now(), true), Decision::Nothing);
        w.sync(Some(&path)); // "keep mine"
        clock.advance(Duration::from_secs(1));
        assert_eq!(w.poll(clock.now(), true), Decision::Nothing);
    }
}
