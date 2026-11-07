//! Software rendering for the window (§7.2). Some graphics drivers cannot
//! compile vello's shaders (nouveau on older NVIDIA GPUs), so the canvas
//! stays blank. The canvas then offers to restart on Mesa's CPU renderer,
//! and the choice is remembered in `$XDG_CONFIG_HOME/tessel/gpu`.
//!
//! `TESSEL_GPU=software` forces the CPU renderer for one run and
//! `TESSEL_GPU=hardware` ignores the remembered choice.

use std::path::PathBuf;

/// The environment variable that overrides the remembered choice.
pub const GPU_ENV: &str = "TESSEL_GPU";

const SOFTWARE: &str = "software";
const MESA_EGL: &str = "/usr/share/glvnd/egl_vendor.d/50_mesa.json";

/// `$XDG_CONFIG_HOME/tessel/gpu`, else `~/.config/tessel/gpu`.
pub fn pref_path() -> Option<PathBuf> {
    crate::panels::config_path().map(|p| p.with_file_name("gpu"))
}

/// Whether software rendering was chosen and remembered.
pub fn software_saved() -> bool {
    pref_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|s| s.trim() == SOFTWARE)
}

/// Remember software rendering for later launches.
pub fn save_software() -> std::io::Result<()> {
    let path = pref_path().ok_or_else(|| std::io::Error::other("no config directory"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{SOFTWARE}\n"))
}

/// Whether this run should render on the CPU: `TESSEL_GPU` if set, else
/// the remembered choice.
pub fn software_requested() -> bool {
    match std::env::var(GPU_ENV) {
        Ok(v) if v.eq_ignore_ascii_case(SOFTWARE) => true,
        Ok(v) if v.eq_ignore_ascii_case("hardware") => false,
        _ => software_saved(),
    }
}

/// Set up the environment for Mesa's CPU OpenGL renderer (llvmpipe), which
/// every Mesa install has. Call before the window and its GPU device are
/// made, while the process has one thread. Variables already set win.
pub fn apply(software: bool) {
    if !software {
        return;
    }
    let set_default = |key: &str, value: &str| {
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
        }
    };
    set_default("WGPU_BACKEND", "gl");
    set_default("LIBGL_ALWAYS_SOFTWARE", "true");
    // With NVIDIA's own driver installed, glvnd may pick its EGL library,
    // which ignores LIBGL_ALWAYS_SOFTWARE; point it at Mesa's instead.
    if std::path::Path::new(MESA_EGL).exists() {
        set_default("__EGL_VENDOR_LIBRARY_FILENAMES", MESA_EGL);
    }
    log::info!("rendering on the CPU (software); {GPU_ENV}=hardware to use the GPU");
}

/// Start a new Tessel on `file` with software rendering. The caller closes
/// this window afterwards.
pub fn relaunch_software(file: Option<&std::path::Path>) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.env(GPU_ENV, SOFTWARE);
    if let Some(f) = file {
        cmd.arg(f);
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test, because it changes process-wide environment variables.
    #[test]
    fn remembered_choice_and_env_override() {
        let dir = std::env::temp_dir().join(format!("tessel-gpu-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        std::env::remove_var(GPU_ENV);
        assert!(!software_saved());
        assert!(!software_requested());

        save_software().unwrap();
        assert_eq!(pref_path().unwrap(), dir.join("tessel").join("gpu"));
        assert!(software_saved());
        assert!(software_requested());

        std::env::set_var(GPU_ENV, "hardware");
        assert!(!software_requested());
        std::env::set_var(GPU_ENV, "SOFTWARE");
        assert!(software_requested());

        std::env::remove_var(GPU_ENV);
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
