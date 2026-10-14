//! Where agents find the app (§6.1): `$XDG_RUNTIME_DIR/tessel/`
//! holds one `<pid>.sock` per window (mode 0600, in a 0700 directory), a
//! `<pid>.file` naming the document it has open, and a `current.sock`
//! symlink to the most recently focused window.

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

/// The symlink to the most recently focused window's socket.
pub const CURRENT: &str = "current.sock";

/// `$XDG_RUNTIME_DIR/tessel`, or a per-user directory under the
/// temp dir when there is no runtime dir.
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d).join("tessel"),
        None => {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            std::env::temp_dir().join(format!("tessel-{user}"))
        }
    }
}

pub fn socket_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.sock"))
}

fn file_marker(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.file"))
}

/// Whether a process with this pid is running.
pub fn pid_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

/// The pids that have a socket in `dir`, live or not.
pub fn socket_pids(dir: &Path) -> Vec<u32> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = entries
        .filter_map(|e| {
            let name = e.ok()?.file_name();
            name.to_str()?.strip_suffix(".sock")?.parse().ok()
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// The document a window has open, as it wrote it in `<pid>.file`.
pub fn window_file(dir: &Path, pid: u32) -> Option<PathBuf> {
    let text = fs::read_to_string(file_marker(dir, pid)).ok()?;
    let text = text.trim_end_matches('\n');
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// Remove the sockets and markers of windows whose process is gone, and a
/// `current.sock` that points at nothing.
pub fn remove_stale(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        let pid = name
            .strip_suffix(".sock")
            .or_else(|| name.strip_suffix(".file"))
            .and_then(|p| p.parse::<u32>().ok());
        if let Some(pid) = pid {
            if !pid_alive(pid) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let current = dir.join(CURRENT);
    if fs::symlink_metadata(&current).is_ok() && fs::metadata(&current).is_err() {
        let _ = fs::remove_file(current);
    }
}

/// Create `dir` with mode 0700, or tighten it to 0700 if it exists and is
/// ours. Fails if it is not ours (see [`check_private_dir`]).
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let uid = current_uid()?;
    check_dir(dir, uid, false)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    check_dir(dir, uid, true)
}

/// The uid this process runs as: the owner of `/proc/self`.
pub fn current_uid() -> io::Result<u32> {
    Ok(fs::metadata("/proc/self")?.uid())
}

/// Refuse a socket directory another local user could have planted a fake
/// window in: it must be a directory (not a symlink), owned by us, with no
/// group or other permissions. Without `$XDG_RUNTIME_DIR` the directory is
/// a predictable path under the temp dir that anyone can create first.
/// A missing directory fails with `NotFound`.
pub fn check_private_dir(dir: &Path) -> io::Result<()> {
    check_dir(dir, current_uid()?, true)
}

fn check_dir(dir: &Path, uid: u32, check_mode: bool) -> io::Result<()> {
    let refuse = |why: String| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing socket directory {}: {why}", dir.display()),
        ))
    };
    let m = fs::symlink_metadata(dir)?;
    if m.file_type().is_symlink() {
        return refuse("it is a symlink".into());
    }
    if !m.is_dir() {
        return refuse("it is not a directory".into());
    }
    if m.uid() != uid {
        return refuse(format!(
            "it is owned by uid {}, not by you (uid {uid})",
            m.uid()
        ));
    }
    let mode = m.mode() & 0o777;
    if check_mode && mode & 0o077 != 0 {
        return refuse(format!(
            "its mode is {mode:03o}; other users must have no access (0700)"
        ));
    }
    Ok(())
}

/// This window's socket and markers; removed when dropped.
pub struct SocketFiles {
    dir: PathBuf,
    pid: u32,
}

impl SocketFiles {
    /// Bind `<pid>.sock` in `dir` (mode 0600), after clearing out stale
    /// sockets of dead processes.
    pub fn bind(dir: &Path, pid: u32) -> io::Result<(SocketFiles, UnixListener)> {
        ensure_private_dir(dir)?;
        remove_stale(dir);
        let path = socket_path(dir, pid);
        // A socket file left behind by an earlier process with our pid.
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok((
            SocketFiles {
                dir: dir.to_path_buf(),
                pid,
            },
            listener,
        ))
    }

    pub fn path(&self) -> PathBuf {
        socket_path(&self.dir, self.pid)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Point `current.sock` at this window (it gained focus). The link is
    /// replaced atomically, so a bridge never sees it missing.
    pub fn make_current(&self) -> io::Result<()> {
        let tmp = self.dir.join(format!(".{}.current", self.pid));
        let _ = fs::remove_file(&tmp);
        std::os::unix::fs::symlink(format!("{}.sock", self.pid), &tmp)?;
        fs::rename(&tmp, self.dir.join(CURRENT))
    }

    /// Whether `current.sock` points at this window.
    pub fn is_current(&self) -> bool {
        fs::read_link(self.dir.join(CURRENT))
            .is_ok_and(|t| t == Path::new(&format!("{}.sock", self.pid)))
    }

    /// Record which document this window has open, for `mcp --file`.
    pub fn set_file(&self, file: Option<&Path>) {
        let marker = file_marker(&self.dir, self.pid);
        let result = match file.map(|f| f.canonicalize().unwrap_or_else(|_| f.to_path_buf())) {
            Some(f) => fs::write(&marker, format!("{}\n", f.display())),
            None => fs::remove_file(&marker).or_else(|e| match e.kind() {
                io::ErrorKind::NotFound => Ok(()),
                _ => Err(e),
            }),
        };
        if let Err(e) = result {
            tracing::warn!("{}: {e}", marker.display());
        }
    }
}

impl Drop for SocketFiles {
    fn drop(&mut self) {
        if self.is_current() {
            let _ = fs::remove_file(self.dir.join(CURRENT));
        }
        let _ = fs::remove_file(self.path());
        let _ = fs::remove_file(file_marker(&self.dir, self.pid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("god-sock-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    /// A pid that is not running.
    fn dead_pid() -> u32 {
        (4_000_000..).find(|p| !pid_alive(*p)).unwrap()
    }

    #[test]
    fn bind_sets_modes_links_current_and_cleans_up() {
        let root = temp("bind");
        let dir = root.join("tessel");
        let me = std::process::id();
        let dead = dead_pid();
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(socket_path(&dir, dead), "").unwrap();
        fs::write(file_marker(&dir, dead), "/x\n").unwrap();
        std::os::unix::fs::symlink(format!("{dead}.sock"), dir.join(CURRENT)).unwrap();

        let (files, _listener) = SocketFiles::bind(&dir, me).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&files.path()), 0o600);
        assert!(!socket_path(&dir, dead).exists(), "stale socket removed");
        assert!(!file_marker(&dir, dead).exists());
        assert!(
            fs::symlink_metadata(dir.join(CURRENT)).is_err(),
            "dangling current.sock removed once its target is gone"
        );
        assert_eq!(socket_pids(&dir), vec![me]);

        files.make_current().unwrap();
        assert!(files.is_current());
        assert_eq!(
            fs::canonicalize(dir.join(CURRENT)).unwrap(),
            fs::canonicalize(files.path()).unwrap()
        );
        let doc = root.join("a.tessel.json");
        fs::write(&doc, "{}").unwrap();
        files.set_file(Some(&doc));
        assert_eq!(window_file(&dir, me), Some(doc.canonicalize().unwrap()));

        drop(files);
        assert!(!socket_path(&dir, me).exists());
        assert!(window_file(&dir, me).is_none());
        assert!(fs::symlink_metadata(dir.join(CURRENT)).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn socket_directories_must_be_private_and_ours() {
        let root = temp("private");
        let dir = root.join("tessel");
        assert_eq!(
            check_private_dir(&dir).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::create_dir_all(&dir).unwrap();
        let chmod = |p: &Path, m| fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
        for mode in [0o755, 0o750, 0o701, 0o770, 0o777] {
            chmod(&dir, mode);
            let e = check_private_dir(&dir).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
            assert!(e.to_string().contains("mode"), "{e}");
        }
        chmod(&dir, 0o700);
        check_private_dir(&dir).unwrap();

        let uid = current_uid().unwrap();
        let e = check_dir(&dir, uid + 1, true).unwrap_err();
        assert!(e.to_string().contains("owned by uid"), "{e}");
        // `ensure_private_dir` tightens our own directory but cannot claim
        // someone else's.
        assert!(check_dir(&dir, uid + 1, false).is_err());

        let link = root.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let e = check_private_dir(&link).unwrap_err();
        assert!(e.to_string().contains("symlink"), "{e}");
        assert!(
            ensure_private_dir(&link).is_err(),
            "a planted symlink is not used"
        );
        assert!(SocketFiles::bind(&link, std::process::id()).is_err());

        let file = root.join("file");
        fs::write(&file, "").unwrap();
        assert!(check_private_dir(&file).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
