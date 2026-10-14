//! Where the left and right side panels live: docked in the main window,
//! hidden, or detached into their own native window. The mode state machine
//! and its small JSON file (`$XDG_CONFIG_HOME/tessel/ui.json`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelMode {
    Docked,
    Hidden,
    Detached,
}

impl PanelMode {
    fn name(self) -> &'static str {
        match self {
            PanelMode::Docked => "docked",
            PanelMode::Hidden => "hidden",
            PanelMode::Detached => "detached",
        }
    }

    fn parse(s: &str) -> Option<PanelMode> {
        match s {
            "docked" => Some(PanelMode::Docked),
            "hidden" => Some(PanelMode::Hidden),
            "detached" => Some(PanelMode::Detached),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// One panel's mode and remembered size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelState {
    pub mode: PanelMode,
    /// What Show restores after a Hide: `Docked` or `Detached`.
    shown_mode: PanelMode,
    /// Docked: the width. Detached: the window's inner size (width, height).
    pub docked_width: Option<f32>,
    pub window_size: Option<[f32; 2]>,
}

impl PanelState {
    fn new(mode: PanelMode) -> PanelState {
        PanelState {
            mode,
            shown_mode: if mode == PanelMode::Hidden {
                PanelMode::Docked
            } else {
                mode
            },
            docked_width: None,
            window_size: None,
        }
    }

    pub fn shown(&self) -> bool {
        self.mode != PanelMode::Hidden
    }

    pub fn set(&mut self, mode: PanelMode) {
        if mode != PanelMode::Hidden {
            self.shown_mode = mode;
        }
        self.mode = mode;
    }

    pub fn toggle(&mut self) {
        let next = if self.shown() {
            PanelMode::Hidden
        } else {
            self.shown_mode
        };
        self.set(next);
    }

    fn to_json(self) -> Value {
        json!({
            "mode": self.mode.name(),
            "docked_width": self.docked_width,
            "window_size": self.window_size,
        })
    }

    fn from_json(v: &Value) -> PanelState {
        let mode = v
            .get("mode")
            .and_then(Value::as_str)
            .and_then(PanelMode::parse)
            .unwrap_or(PanelMode::Docked);
        let mut s = PanelState::new(mode);
        let ok = |x: f64| x.is_finite() && x > 0.0;
        s.docked_width = v
            .get("docked_width")
            .and_then(Value::as_f64)
            .filter(|w| ok(*w))
            .map(|w| w as f32);
        s.window_size = v
            .get("window_size")
            .and_then(Value::as_array)
            .filter(|a| a.len() == 2)
            .and_then(|a| Some([a[0].as_f64()?, a[1].as_f64()?]))
            .filter(|[w, h]| ok(*w) && ok(*h))
            .map(|[w, h]| [w as f32, h as f32]);
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panels {
    pub left: PanelState,
    pub right: PanelState,
}

impl Default for Panels {
    fn default() -> Panels {
        Panels {
            left: PanelState::new(PanelMode::Docked),
            right: PanelState::new(PanelMode::Docked),
        }
    }
}

impl Panels {
    pub fn get(&self, side: Side) -> &PanelState {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    pub fn get_mut(&mut self, side: Side) -> &mut PanelState {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }

    /// Ctrl+\: hide both if either is shown, else restore both.
    pub fn toggle_all(&mut self) {
        if self.left.shown() || self.right.shown() {
            self.left.set(PanelMode::Hidden);
            self.right.set(PanelMode::Hidden);
        } else {
            self.left.toggle();
            self.right.toggle();
        }
    }

    /// The user closed a detached window: the panel docks back.
    pub fn close_detached(&mut self, side: Side) {
        let p = self.get_mut(side);
        if p.mode == PanelMode::Detached {
            p.set(PanelMode::Docked);
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "left": self.left.to_json(), "right": self.right.to_json() })
    }

    /// Anything unreadable falls back to the defaults.
    pub fn from_json(v: &Value) -> Panels {
        let mut p = Panels::default();
        if let Some(l) = v.get("left") {
            p.left = PanelState::from_json(l);
        }
        if let Some(r) = v.get("right") {
            p.right = PanelState::from_json(r);
        }
        p
    }

    pub fn load(path: &Path) -> Panels {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| Panels::from_json(&v))
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_default();
        std::fs::write(path, text)
    }
}

/// `$XDG_CONFIG_HOME/tessel/ui.json`, else `~/.config/...`.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("tessel").join("ui.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("god-panels-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("ui.json")
    }

    #[test]
    fn toggle_all_hides_when_any_shown_then_restores() {
        let mut p = Panels::default();
        p.right.set(PanelMode::Hidden);
        p.toggle_all();
        assert!(!p.left.shown() && !p.right.shown());
        p.toggle_all();
        assert_eq!(p.left.mode, PanelMode::Docked);
        assert_eq!(p.right.mode, PanelMode::Docked);
    }

    #[test]
    fn restore_remembers_detached() {
        let mut p = Panels::default();
        p.left.set(PanelMode::Detached);
        p.toggle_all();
        assert_eq!(p.left.mode, PanelMode::Hidden);
        p.toggle_all();
        assert_eq!(p.left.mode, PanelMode::Detached);
        assert_eq!(p.right.mode, PanelMode::Docked);
    }

    #[test]
    fn detach_then_close_docks() {
        let mut p = Panels::default();
        p.right.set(PanelMode::Detached);
        p.close_detached(Side::Right);
        assert_eq!(p.right.mode, PanelMode::Docked);
        p.close_detached(Side::Left);
        assert_eq!(p.left.mode, PanelMode::Docked);
        p.left.set(PanelMode::Hidden);
        p.close_detached(Side::Left);
        assert_eq!(p.left.mode, PanelMode::Hidden);
    }

    #[test]
    fn round_trip() {
        let path = tmp("rt");
        let mut p = Panels::default();
        p.left.set(PanelMode::Detached);
        p.left.window_size = Some([300.0, 500.0]);
        p.right.docked_width = Some(333.0);
        p.right.set(PanelMode::Hidden);
        p.save(&path).unwrap();
        let q = Panels::load(&path);
        assert_eq!(q.left.mode, PanelMode::Detached);
        assert_eq!(q.left.window_size, Some([300.0, 500.0]));
        assert_eq!(q.right.docked_width, Some(333.0));
        assert_eq!(q.right.mode, PanelMode::Hidden);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_or_corrupt_gives_defaults() {
        let path = tmp("bad");
        assert_eq!(Panels::load(&path), Panels::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Panels::load(&path), Panels::default());
        std::fs::write(&path, r#"{"left":{"mode":"sideways","docked_width":-4}}"#).unwrap();
        assert_eq!(Panels::load(&path), Panels::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
