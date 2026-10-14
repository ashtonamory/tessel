//! History helpers: an injectable clock, nudge merging, transaction labels
//! and the "Activity" list (§5.3).

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use doc::{History, NodeId, Origin};

/// Arrow-key nudges closer together than this merge into one undo step.
pub const NUDGE_MERGE_WINDOW: Duration = Duration::from_secs(1);

/// How many transactions the Activity list shows.
const ACTIVITY_LIMIT: usize = 100;

/// The time source for history stamps, nudge merging, autosave and file
/// polling. `manual` clocks only move when told to, so tests need no sleeps.
#[derive(Clone, Debug)]
pub struct Clock {
    base: Instant,
    offset: Option<Rc<Cell<Duration>>>,
}

impl Default for Clock {
    fn default() -> Self {
        Clock::system()
    }
}

impl Clock {
    pub fn system() -> Clock {
        Clock {
            base: Instant::now(),
            offset: None,
        }
    }

    /// A clock that stands still until `advance`d. Clones share the time.
    pub fn manual() -> Clock {
        Clock {
            base: Instant::now(),
            offset: Some(Rc::new(Cell::new(Duration::ZERO))),
        }
    }

    pub fn now(&self) -> Instant {
        match &self.offset {
            Some(o) => self.base + o.get(),
            None => Instant::now(),
        }
    }

    /// Move a manual clock forward; does nothing on the system clock.
    pub fn advance(&self, by: Duration) {
        if let Some(o) = &self.offset {
            o.set(o.get() + by);
        }
    }
}

/// The last nudge, to decide whether the next one merges into it.
#[derive(Clone, Debug)]
pub struct NudgeState {
    pub selection: Vec<NodeId>,
    pub at: Instant,
    /// The editor revision right after that nudge; any other edit, undo or
    /// redo since then changes it and ends the merge.
    pub revision: u64,
}

impl NudgeState {
    pub fn can_merge(&self, selection: &[NodeId], revision: u64, now: Instant) -> bool {
        self.revision == revision
            && self.selection == selection
            && now.saturating_duration_since(self.at) <= NUDGE_MERGE_WINDOW
    }
}

/// "Move layer" / "Move 3 layers".
pub fn layers_label(verb: &str, n: usize) -> String {
    if n == 1 {
        format!("{verb} layer")
    } else {
        format!("{verb} {n} layers")
    }
}

/// Give a gesture label its layer count when it has none yet.
pub fn refine_gesture_label(label: &str, selected: usize) -> String {
    match label {
        "Resize" | "Rotate" if selected > 0 => layers_label(label, selected),
        _ => label.to_string(),
    }
}

/// One row of the Activity list.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityEntry {
    pub label: String,
    pub origin: Origin,
    pub age: Duration,
}

impl ActivityEntry {
    /// "Agent: claude" for agents, `None` for the user.
    pub fn tag(&self) -> Option<String> {
        match &self.origin {
            Origin::User => None,
            Origin::Agent { client } => Some(format!("Agent: {client}")),
        }
    }
}

/// The recent transactions, newest first.
pub fn entries(history: &History, now: Instant) -> Vec<ActivityEntry> {
    history
        .transactions()
        .iter()
        .rev()
        .take(ACTIVITY_LIMIT)
        .map(|t| ActivityEntry {
            label: t.label.clone(),
            origin: t.origin.clone(),
            age: now.saturating_duration_since(t.at),
        })
        .collect()
}

/// "just now", "12s ago", "3m ago", "2h ago".
pub fn format_age(age: Duration) -> String {
    match age.as_secs() {
        0..=4 => "just now".to_string(),
        s @ 5..=59 => format!("{s}s ago"),
        s @ 60..=3599 => format!("{}m ago", s / 60),
        s => format!("{}h ago", s / 3600),
    }
}

/// The collapsible Activity panel.
#[derive(Default)]
pub struct ActivityPanel {
    pub open: bool,
}

impl ActivityPanel {
    pub fn ui(&mut self, ui: &mut egui::Ui, history: &History, now: Instant) {
        let resp = egui::CollapsingHeader::new("Activity")
            .id_salt("activity")
            .open(Some(self.open))
            .show(ui, |ui| {
                let rows = entries(history, now);
                if rows.is_empty() {
                    ui.weak("No edits yet");
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for e in rows {
                            ui.horizontal(|ui| {
                                ui.label(&e.label);
                                if let Some(tag) = e.tag() {
                                    ui.colored_label(ui.visuals().hyperlink_color, tag);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| ui.weak(format_age(e.age)),
                                );
                            });
                        }
                    });
            });
        if resp.header_response.clicked() {
            self.open = !self.open;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_only_moves_when_advanced() {
        let c = Clock::manual();
        let a = c.now();
        assert_eq!(c.now(), a);
        let c2 = c.clone();
        c2.advance(Duration::from_millis(1500));
        assert_eq!(c.now() - a, Duration::from_millis(1500));
    }

    #[test]
    fn nudge_window_is_one_second_on_the_same_selection() {
        let c = Clock::manual();
        let id = doc::Id::new(doc::IdKind::Node);
        let st = NudgeState {
            selection: vec![id.clone()],
            at: c.now(),
            revision: 3,
        };
        c.advance(Duration::from_millis(900));
        assert!(st.can_merge(std::slice::from_ref(&id), 3, c.now()));
        assert!(!st.can_merge(std::slice::from_ref(&id), 4, c.now()));
        assert!(!st.can_merge(&[], 3, c.now()));
        c.advance(Duration::from_millis(200));
        assert!(!st.can_merge(&[id], 3, c.now()));
    }

    #[test]
    fn labels_and_ages() {
        assert_eq!(layers_label("Move", 1), "Move layer");
        assert_eq!(layers_label("Move", 3), "Move 3 layers");
        assert_eq!(refine_gesture_label("Resize", 2), "Resize 2 layers");
        assert_eq!(refine_gesture_label("Move layer", 2), "Move layer");
        assert_eq!(format_age(Duration::from_secs(2)), "just now");
        assert_eq!(format_age(Duration::from_secs(30)), "30s ago");
        assert_eq!(format_age(Duration::from_secs(180)), "3m ago");
        assert_eq!(format_age(Duration::from_secs(7300)), "2h ago");
    }
}
