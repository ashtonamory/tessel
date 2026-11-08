//! A screenshot of the preview (§13.4 `preview_screenshot`): the page served
//! by a server of its own and captured by headless Chromium, so the agent
//! sees what the browser shows (motion included) without a window opening.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{which, Page, Server};

/// Chromium builds that run headless. Brave is left out: it is not always
/// happy headless.
const HEADLESS: [&str; 4] = [
    "chromium",
    "google-chrome-stable",
    "google-chrome",
    "chromium-browser",
];

/// The largest viewport side, px.
pub const MAX_SIDE: u32 = 4096;

/// What to capture.
#[derive(Clone, Debug, PartialEq)]
pub struct ShotOptions {
    /// Viewport size, px.
    pub width: u32,
    pub height: u32,
    /// How far down the page is scrolled, px.
    pub scroll: u32,
    /// Capture as a browser set to reduce motion: no animations or
    /// transitions, every element at rest.
    pub reduced_motion: bool,
    /// How long the page runs before the capture (virtual time).
    pub settle: Duration,
    /// Give up on Chromium after this long (wall time).
    pub timeout: Duration,
}

impl Default for ShotOptions {
    fn default() -> Self {
        ShotOptions {
            width: 1440,
            height: 900,
            scroll: 0,
            reduced_motion: false,
            settle: Duration::from_millis(1500),
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ShotError {
    #[error("no Chromium-based browser found (tried {})", HEADLESS.join(", "))]
    NoBrowser,
    #[error("{0}")]
    Failed(String),
}

/// The headless browser used, if one is installed. `$CHROMIUM` wins.
pub fn headless_browser() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CHROMIUM").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
        return which(&p.to_string_lossy());
    }
    HEADLESS.iter().find_map(|b| which(b))
}

/// Capture `page` as a PNG. Blocks for up to `opts.timeout`.
pub fn screenshot(page: Page, opts: &ShotOptions) -> Result<Vec<u8>, ShotError> {
    let browser = headless_browser().ok_or(ShotError::NoBrowser)?;
    if opts.width == 0 || opts.height == 0 || opts.width > MAX_SIDE || opts.height > MAX_SIDE {
        return Err(ShotError::Failed(format!(
            "the viewport must be 1 to {MAX_SIDE} px on each side"
        )));
    }
    let server = Server::start().map_err(|e| ShotError::Failed(format!("preview server: {e}")))?;
    server.publish(page);
    let dir = TempDir::new().map_err(|e| ShotError::Failed(format!("temp folder: {e}")))?;
    let out = dir.0.join("shot.png");
    let url = format!(
        "{}?live=0&chrome=0&w={}&h={}&scroll={}&reduced={}",
        server.url(),
        opts.width,
        opts.height,
        opts.scroll,
        u8::from(opts.reduced_motion)
    );
    let mut cmd = Command::new(&browser);
    cmd.args([
        "--headless",
        "--disable-gpu",
        "--hide-scrollbars",
        "--no-first-run",
        "--no-default-browser-check",
        "--mute-audio",
    ])
    .arg(format!(
        "--user-data-dir={}",
        dir.0.join("profile").display()
    ))
    .arg(format!("--window-size={},{}", opts.width, opts.height))
    .arg(format!("--virtual-time-budget={}", opts.settle.as_millis()))
    .arg(format!("--screenshot={}", out.display()));
    if opts.reduced_motion {
        cmd.arg("--force-prefers-reduced-motion");
    }
    // Chromium's log goes to a file: a pipe nobody reads could fill and stall it.
    let log = std::fs::File::create(dir.0.join("log"))
        .map_err(|e| ShotError::Failed(format!("temp folder: {e}")))?;
    cmd.arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    let mut child = cmd
        .spawn()
        .map_err(|e| ShotError::Failed(format!("{}: {e}", browser.display())))?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < opts.timeout => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ShotError::Failed(format!(
                    "the browser took longer than {} s",
                    opts.timeout.as_secs()
                )));
            }
            Err(e) => return Err(ShotError::Failed(e.to_string())),
        }
    };
    match std::fs::read(&out) {
        Ok(png) if !png.is_empty() => Ok(png),
        _ => {
            let err = std::fs::read_to_string(dir.0.join("log")).unwrap_or_default();
            let last = err
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            Err(ShotError::Failed(format!(
                "the browser wrote no screenshot ({status}){}{last}",
                if last.is_empty() { "" } else { ": " }
            )))
        }
    }
}

/// A folder removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<TempDir> {
        let p = std::env::temp_dir().join(format!("tessel-shot-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&p)?;
        Ok(TempDir(p))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(Path::new(&self.0));
    }
}
