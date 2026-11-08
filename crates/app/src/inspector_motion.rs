//! The inspector's Motion section (§13.3): a node's scroll, hover and loop
//! effects. They play in the browser preview and the code export, not on
//! the canvas, so the section ends with a button that opens the preview.
//! A child module of `inspector`: same lenses and commit path, one undo
//! step per edit.

use doc::{ColorRef, Document, LoopEffect, Motion, Node, NodeKind, RevealEffect};
use egui::Ui;

use super::{shared, Inspector, Lens, Num, Shared, PX};

/// Effects in the order the menu lists them: (kind, label).
const KINDS: [(&str, &str); 4] = [
    ("reveal", "Reveal on scroll"),
    ("parallax", "Parallax"),
    ("hover", "Hover"),
    ("loop", "Loop"),
];

const MS: Num = Num {
    speed: 10.0,
    step: 10.0,
    min: 0.0,
    max: 10_000.0,
    scale: 1.0,
    suffix: " ms",
    decimals: 0,
};
const LOOP_MS: Num = Num {
    step: 50.0,
    speed: 20.0,
    min: 100.0,
    max: 60_000.0,
    ..MS
};
const SPEED: Num = Num {
    speed: 0.01,
    step: 0.05,
    min: -1.0,
    max: 2.0,
    scale: 1.0,
    suffix: "×",
    decimals: 2,
};
const DISTANCE: Num = Num {
    min: 0.0,
    max: 2000.0,
    ..PX
};
const LIFT: Num = Num {
    min: -200.0,
    max: 200.0,
    ..PX
};
/// A scale factor as a percentage.
const SCALE: Num = Num {
    speed: 0.5,
    step: 1.0,
    min: 0.1,
    max: 4.0,
    scale: 100.0,
    suffix: "%",
    decimals: 0,
};
const OPACITY: Num = Num {
    min: 0.0,
    max: 1.0,
    ..SCALE
};
const FLOAT: Num = Num {
    min: 0.0,
    max: 500.0,
    ..PX
};
const TURNS: Num = Num {
    speed: 0.01,
    step: 0.25,
    min: -10.0,
    max: 10.0,
    scale: 1.0,
    suffix: " turn",
    decimals: 2,
};

/// The effect `kind` with values that read well on most designs.
fn new_effect(kind: &str) -> Motion {
    match kind {
        "reveal" => Motion::Reveal {
            effect: RevealEffect::SlideUp,
            distance: 24.0,
        },
        "parallax" => Motion::Parallax { speed: 0.5 },
        "hover" => Motion::Hover {
            scale: 1.05,
            lift: 2.0,
            opacity: None,
            fill: None,
            duration_ms: 150,
        },
        _ => Motion::Loop {
            effect: LoopEffect::Float,
            amount: 8.0,
            duration_ms: 2000,
        },
    }
}

fn label_of(kind: &str) -> &'static str {
    match kind {
        "reveal" => "Reveal",
        "parallax" => "Parallax",
        "hover" => "Hover",
        _ => "Loop",
    }
}

/// Every selected node can have motion: no groups.
pub(super) fn applies(nodes: &[&Node]) -> bool {
    nodes.iter().all(|n| !matches!(n.kind, NodeKind::Group(_)))
}

impl Inspector {
    pub(super) fn motion(&mut self, ui: &mut Ui, doc: &Document, nodes: &[&Node]) {
        let list: Lens<Vec<Motion>> = Lens::new(|n| Some(n.motion.clone()), |n, v| n.motion = v);
        let value = shared(nodes, &list);
        ui.horizontal(|ui| {
            ui.strong("Motion");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let present: Vec<&str> = match &value {
                    Shared::Same(l) => l.iter().map(Motion::kind).collect(),
                    _ => Vec::new(),
                };
                let resp = ui
                    .menu_button("+", |ui| {
                        for (kind, label) in KINDS {
                            let has = present.contains(&kind);
                            let b = ui.add_enabled(!has, egui::Button::new(label));
                            self.fields.push((format!("motion.add.{kind}"), b.rect));
                            if b.clicked() {
                                let m = new_effect(kind);
                                let edit = match &value {
                                    Shared::Same(_) => list.modify(move |l| l.push(m.clone())),
                                    _ => list.to(vec![m]),
                                };
                                let what = format!("Add {} motion", label_of(kind).to_lowercase());
                                self.push("motion.add", &what, false, edit);
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Add a motion effect (plays in the browser preview and export)");
                self.fields.push(("motion.add".into(), resp.rect));
            });
        });
        let effects = match value {
            Shared::Same(l) => l,
            Shared::Mixed => {
                ui.weak("Mixed — + replaces them");
                return;
            }
            Shared::Absent => return,
        };
        if effects.is_empty() {
            ui.weak("None. Effects play in the browser preview, not here.");
            return;
        }
        for m in &effects {
            self.effect(ui, doc, nodes, &list, m);
        }
        let resp = ui
            .button("Preview in browser")
            .on_hover_text("Open the frame in the preview window (Ctrl+Shift+P)");
        self.fields.push(("motion.preview".into(), resp.rect));
        if resp.clicked() {
            self.preview = true;
        }
    }

    /// The rows for one effect.
    fn effect(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        nodes: &[&Node],
        list: &Lens<Vec<Motion>>,
        m: &Motion,
    ) {
        let kind = m.kind();
        let k = format!("motion.{kind}");
        let eff: Lens<Motion> = list.map(
            move |l| l.iter().find(|m| m.kind() == kind).cloned(),
            move |l, m| {
                if let Some(slot) = l.iter_mut().find(|x| x.kind() == kind) {
                    *slot = m;
                }
            },
        );
        let label = format!("Change {} motion", label_of(kind).to_lowercase());
        let label = label.as_str();
        ui.horizontal(|ui| {
            ui.label(label_of(kind));
            match m {
                Motion::Reveal { effect, .. } => {
                    let options = [
                        (RevealEffect::Fade, "Fade"),
                        (RevealEffect::SlideUp, "Slide up"),
                        (RevealEffect::SlideDown, "Slide down"),
                        (RevealEffect::SlideLeft, "Slide left"),
                        (RevealEffect::SlideRight, "Slide right"),
                        (RevealEffect::Scale, "Scale"),
                    ];
                    let name = options.iter().find(|o| o.0 == *effect).map_or("", |o| o.1);
                    let resp = egui::ComboBox::from_id_salt(format!("{k}.effect"))
                        .width(84.0)
                        .selected_text(name)
                        .show_ui(ui, |ui| {
                            for (v, text) in options {
                                if ui.selectable_label(v == *effect, text).clicked() && v != *effect
                                {
                                    let e = eff.modify(move |m| {
                                        if let Motion::Reveal { effect, .. } = m {
                                            *effect = v;
                                        }
                                    });
                                    self.push(&format!("{k}.effect"), label, false, e);
                                }
                            }
                        });
                    self.fields
                        .push((format!("{k}.effect"), resp.response.rect));
                    if *effect != RevealEffect::Fade && *effect != RevealEffect::Scale {
                        let d = eff.map(
                            |m| match m {
                                Motion::Reveal { distance, .. } => Some(*distance),
                                _ => None,
                            },
                            |m, v| {
                                if let Motion::Reveal { distance, .. } = m {
                                    *distance = v;
                                }
                            },
                        );
                        self.number_w(
                            ui,
                            nodes,
                            &format!("{k}.distance"),
                            label,
                            d,
                            DISTANCE,
                            44.0,
                        );
                    }
                }
                Motion::Parallax { .. } => {
                    let s = eff.map(
                        |m| match m {
                            Motion::Parallax { speed } => Some(*speed),
                            _ => None,
                        },
                        |m, v| {
                            if let Motion::Parallax { speed } = m {
                                *speed = v;
                            }
                        },
                    );
                    self.number_w(ui, nodes, &format!("{k}.speed"), label, s, SPEED, 52.0)
                }
                Motion::Hover { .. } => {
                    let scale = eff.map(
                        |m| match m {
                            Motion::Hover { scale, .. } => Some(*scale),
                            _ => None,
                        },
                        |m, v| {
                            if let Motion::Hover { scale, .. } = m {
                                *scale = v;
                            }
                        },
                    );
                    ui.label("Scale");
                    self.number_w(ui, nodes, &format!("{k}.scale"), label, scale, SCALE, 44.0);
                    let lift = eff.map(
                        |m| match m {
                            Motion::Hover { lift, .. } => Some(*lift),
                            _ => None,
                        },
                        |m, v| {
                            if let Motion::Hover { lift, .. } = m {
                                *lift = v;
                            }
                        },
                    );
                    ui.label("Lift");
                    self.number_w(ui, nodes, &format!("{k}.lift"), label, lift, LIFT, 36.0);
                }
                Motion::Loop { effect, .. } => {
                    let options = [
                        (LoopEffect::Float, "Float"),
                        (LoopEffect::Pulse, "Pulse"),
                        (LoopEffect::Spin, "Spin"),
                    ];
                    let name = options.iter().find(|o| o.0 == *effect).map_or("", |o| o.1);
                    let resp = egui::ComboBox::from_id_salt(format!("{k}.effect"))
                        .width(60.0)
                        .selected_text(name)
                        .show_ui(ui, |ui| {
                            for (v, text) in options {
                                if ui.selectable_label(v == *effect, text).clicked() && v != *effect
                                {
                                    // Each effect has its own amount.
                                    let e = eff.to(match (new_effect("loop"), v) {
                                        (Motion::Loop { duration_ms, .. }, LoopEffect::Float) => {
                                            Motion::Loop {
                                                effect: v,
                                                amount: 8.0,
                                                duration_ms,
                                            }
                                        }
                                        (Motion::Loop { duration_ms, .. }, LoopEffect::Pulse) => {
                                            Motion::Loop {
                                                effect: v,
                                                amount: 1.05,
                                                duration_ms,
                                            }
                                        }
                                        _ => Motion::Loop {
                                            effect: v,
                                            amount: 1.0,
                                            duration_ms: 8000,
                                        },
                                    });
                                    self.push(&format!("{k}.effect"), label, false, e);
                                }
                            }
                        });
                    self.fields
                        .push((format!("{k}.effect"), resp.response.rect));
                    let amount = eff.map(
                        |m| match m {
                            Motion::Loop { amount, .. } => Some(*amount),
                            _ => None,
                        },
                        |m, v| {
                            if let Motion::Loop { amount, .. } = m {
                                *amount = v;
                            }
                        },
                    );
                    let num = match effect {
                        LoopEffect::Float => FLOAT,
                        LoopEffect::Pulse => SCALE,
                        LoopEffect::Spin => TURNS,
                    };
                    self.number_w(ui, nodes, &format!("{k}.amount"), label, amount, num, 52.0);
                }
            }
            let resp = ui.add(egui::Button::new("✕").small());
            self.fields.push((format!("{k}.remove"), resp.rect));
            if resp.clicked() {
                let e = list.modify(move |l| l.retain(|m| m.kind() != kind));
                let what = format!("Remove {} motion", label_of(kind).to_lowercase());
                self.push(&k, &what, false, e);
            }
        });
        match m {
            Motion::Hover { opacity, fill, .. } => {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let mut on = opacity.is_some();
                    let resp = ui.checkbox(&mut on, "Opacity");
                    self.fields.push((format!("{k}.opacity.on"), resp.rect));
                    if resp.changed() {
                        let e = eff.modify(move |m| {
                            if let Motion::Hover { opacity, .. } = m {
                                *opacity = on.then_some(0.8);
                            }
                        });
                        self.push(&format!("{k}.opacity"), label, false, e);
                    }
                    if opacity.is_some() {
                        let o = eff.map(
                            |m| match m {
                                Motion::Hover { opacity, .. } => *opacity,
                                _ => None,
                            },
                            |m, v| {
                                if let Motion::Hover { opacity, .. } = m {
                                    *opacity = Some(v);
                                }
                            },
                        );
                        self.number_w(ui, nodes, &format!("{k}.opacity"), label, o, OPACITY, 44.0);
                    }
                    self.duration(ui, nodes, &k, label, &eff, MS);
                });
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let mut on = fill.is_some();
                    let resp = ui.checkbox(&mut on, "Fill");
                    self.fields.push((format!("{k}.fill.on"), resp.rect));
                    if resp.changed() {
                        let e = eff.modify(move |m| {
                            if let Motion::Hover { fill, .. } = m {
                                *fill = on.then(|| ColorRef::literal("#2563eb"));
                            }
                        });
                        self.push(&format!("{k}.fill"), label, false, e);
                    }
                    if fill.is_some() {
                        let c = eff.map(
                            |m| match m {
                                Motion::Hover { fill, .. } => fill.clone(),
                                _ => None,
                            },
                            |m, v| {
                                if let Motion::Hover { fill, .. } = m {
                                    *fill = Some(v);
                                }
                            },
                        );
                        self.color(ui, doc, nodes, &format!("{k}.fill"), label, c);
                    }
                });
            }
            Motion::Loop { .. } => {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    self.duration(ui, nodes, &k, label, &eff, LOOP_MS);
                });
            }
            _ => {}
        }
    }

    /// The effect's `duration_ms`.
    fn duration(
        &mut self,
        ui: &mut Ui,
        nodes: &[&Node],
        k: &str,
        label: &str,
        eff: &Lens<Motion>,
        num: Num,
    ) {
        let d = eff.map(
            |m| match m {
                Motion::Hover { duration_ms, .. } | Motion::Loop { duration_ms, .. } => {
                    Some(*duration_ms as f32)
                }
                _ => None,
            },
            |m, v| {
                if let Motion::Hover { duration_ms, .. } | Motion::Loop { duration_ms, .. } = m {
                    *duration_ms = v.round().max(0.0) as u32;
                }
            },
        );
        ui.label("Time");
        self.number_w(ui, nodes, &format!("{k}.duration"), label, d, num, 64.0);
    }
}
