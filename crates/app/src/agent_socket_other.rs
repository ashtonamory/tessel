//! Where there are no unix sockets (Windows): the window runs, and the link
//! that lets `tessel mcp` reach it is not there yet. `tessel render` and
//! `tessel mcp --headless` do not use it and work as on Linux.

use std::io;
use std::path::{Path, PathBuf};

pub const CURRENT: &str = "current.sock";

/// A folder of this user's own for what lasts one session: the preview's
/// browser profile. `%LOCALAPPDATA%\Tessel\run`, else under the temp dir.
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d).join("Tessel").join("run"),
        None => std::env::temp_dir().join("tessel"),
    }
}

/// Stands in for a window's socket and markers. Never made here.
pub struct SocketFiles {
    dir: PathBuf,
}

impl SocketFiles {
    pub fn bind(_dir: &Path, _pid: u32) -> io::Result<SocketFiles> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "agents cannot connect to a window on this system yet; use `tessel mcp --headless`",
        ))
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(CURRENT)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn make_current(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn is_current(&self) -> bool {
        false
    }

    pub fn set_file(&self, _file: Option<&Path>) {}
}
