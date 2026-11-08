//! Motion effects as CSS (§13.3). No JavaScript: scroll effects are CSS
//! scroll-driven animations, hover is `:hover` with transitions, loops are
//! `@keyframes`.
//!
//! Effects move the individual `translate`, `scale` and `rotate`
//! properties, never `transform`, so they combine with a node's rotation.
//! Effects on the same property stack through `animation-composition:
//! add`, so no wrapper element is needed. A keyframe rule with only a
//! `from` step animates to the element's own value: a reveal ends at the
//! node's opacity and its hover state.

use std::fmt::Write;

use doc::{LoopEffect, Motion, RevealEffect};

use super::ir::{num, px, Val};

/// An exported element that has motion, recorded as it is built.
#[derive(Clone, Debug)]
pub(crate) struct MotionEl {
    /// The element's class (unique in the export).
    pub class: String,
    pub motion: Vec<Motion>,
    /// The node's top edge below the exported root's, and its height, px:
    /// where parallax starts and how far it travels.
    pub top: f64,
    pub height: f64,
    /// `color` for text, else `background-color`: what a hover fill sets.
    pub fill_prop: &'static str,
    /// The hover fill, resolved (tokens kept).
    pub hover_fill: Option<Val>,
}

/// One animation in an element's `animation` list.
#[derive(Clone)]
struct Anim {
    name: String,
    /// `duration timing iteration direction fill`.
    timing: String,
    /// `view()` or `scroll(root)` for scroll effects, `auto` for time.
    timeline: String,
    range: String,
    composition: &'static str,
}

/// The class an element with motion is selected by: its own in HTML, with
/// `prefix` (`motion-`) where utility classes share the namespace.
pub(crate) fn class(prefix: &str, el_class: &str) -> String {
    format!("{prefix}{el_class}")
}

/// The motion stylesheet for `els`, or "" if none has motion. `prefix` is
/// put before each class (see [`class`]); `value` writes a resolved value
/// (tokens as the format refers to them).
pub(crate) fn css(els: &[MotionEl], prefix: &str, value: &dyn Fn(&Val) -> String) -> String {
    if els.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("\n/* Motion (tessel §13.3): CSS only, off for prefers-reduced-motion. */\n");
    let mut scroll_rules = String::new();
    let mut reduced = Vec::new();
    for el in els {
        let cls = class(prefix, &el.class);
        let sel = format!(".{cls}");
        let mut time = Vec::new();
        let mut all = Vec::new();
        let mut transitions = Vec::new();
        let mut hover_motion = Vec::new();
        let mut hover_look = Vec::new();
        // Order matters: reveal replaces, then loop and parallax add on top.
        let mut sorted: Vec<&Motion> = el.motion.iter().collect();
        sorted.sort_by_key(|m| match m {
            Motion::Reveal { .. } => 0,
            Motion::Loop { .. } => 1,
            Motion::Parallax { .. } => 2,
            Motion::Hover { .. } => 3,
        });
        for m in sorted {
            match m {
                Motion::Reveal { effect, distance } => {
                    let name = format!("{cls}-reveal");
                    let d = px(*distance as f64);
                    let from = match effect {
                        RevealEffect::Fade => "opacity: 0;".to_string(),
                        RevealEffect::SlideUp => format!("opacity: 0; translate: 0 {d};"),
                        RevealEffect::SlideDown => format!("opacity: 0; translate: 0 -{d};"),
                        RevealEffect::SlideLeft => format!("opacity: 0; translate: {d} 0;"),
                        RevealEffect::SlideRight => format!("opacity: 0; translate: -{d} 0;"),
                        RevealEffect::Scale => "opacity: 0; scale: 0.9;".to_string(),
                    };
                    let _ = writeln!(out, "\n@keyframes {name} {{\n  from {{ {from} }}\n}}");
                    all.push(Anim {
                        name,
                        timing: "linear backwards".into(),
                        timeline: "view()".into(),
                        range: "entry".into(),
                        composition: "replace",
                    });
                }
                Motion::Loop {
                    effect,
                    amount,
                    duration_ms,
                } => {
                    let name = format!("{cls}-loop");
                    let (to, timing) = match effect {
                        LoopEffect::Float => (
                            format!("translate: 0 {};", px(-(*amount as f64))),
                            "ease-in-out infinite alternate",
                        ),
                        LoopEffect::Pulse => (
                            format!("scale: {};", num(*amount as f64)),
                            "ease-in-out infinite alternate",
                        ),
                        LoopEffect::Spin => (
                            format!("rotate: {}turn;", num(*amount as f64)),
                            "linear infinite",
                        ),
                    };
                    let (from, comp) = match effect {
                        LoopEffect::Float => ("translate: 0 0;", "add"),
                        LoopEffect::Pulse => ("scale: 1;", "replace"),
                        LoopEffect::Spin => ("rotate: 0turn;", "add"),
                    };
                    let _ = writeln!(
                        out,
                        "\n@keyframes {name} {{\n  from {{ {from} }}\n  to {{ {to} }}\n}}"
                    );
                    let anim = Anim {
                        name,
                        timing: format!("{duration_ms}ms {timing}"),
                        timeline: "auto".into(),
                        range: "normal".into(),
                        composition: comp,
                    };
                    time.push(anim.clone());
                    all.push(anim);
                }
                Motion::Parallax { speed } => {
                    // On the page's scroll timeline, from when the element
                    // enters the viewport (scroll 0 if it starts in view, so
                    // the top of the page is the design) until it has left.
                    // Moving down by (1 - speed) of the scroll makes it go at
                    // `speed` times the scroll. Slow ones take longer to
                    // leave: the range is the distance divided by the speed.
                    let k = 1.0 - *speed as f64;
                    if k.abs() < 1e-9 {
                        continue;
                    }
                    let name = format!("{cls}-parallax");
                    let start = format!("max(0px, {} - 100vh)", px(el.top));
                    let travel = format!(
                        "({} - {start}) / {}",
                        px(el.top + el.height),
                        num((*speed as f64).max(0.25))
                    );
                    let _ = writeln!(
                        out,
                        "\n@keyframes {name} {{\n  from {{ translate: 0 0; }}\n  to {{ translate: 0 calc({} * {travel}); }}\n}}",
                        num(k)
                    );
                    all.push(Anim {
                        name,
                        timing: "linear both".into(),
                        timeline: "scroll(root)".into(),
                        range: format!("{start} calc({start} + {travel})"),
                        composition: "add",
                    });
                }
                Motion::Hover {
                    scale,
                    lift,
                    opacity,
                    duration_ms,
                    ..
                } => {
                    let t = format!("{duration_ms}ms ease");
                    if *scale != 1.0 {
                        transitions.push(format!("scale {t}"));
                        hover_motion.push(format!("scale: {};", num(*scale as f64)));
                    }
                    if *lift != 0.0 {
                        transitions.push(format!("translate {t}"));
                        hover_motion.push(format!("translate: 0 {};", px(-(*lift as f64))));
                    }
                    if let Some(o) = opacity {
                        transitions.push(format!("opacity {t}"));
                        hover_look.push(format!("opacity: {};", num(*o as f64)));
                    }
                    if let Some(f) = &el.hover_fill {
                        transitions.push(format!("{} {t}", el.fill_prop));
                        hover_look.push(format!("{}: {};", el.fill_prop, value(f)));
                    }
                }
            }
        }

        // Time-based animations run everywhere; the full list (with scroll
        // timelines) replaces it where scroll-driven animations exist.
        if !time.is_empty() {
            let _ = writeln!(out, "\n{sel} {{");
            write_anims(&mut out, &time, false);
            out.push_str("}\n");
        }
        if all.iter().any(|a| a.timeline != "auto") {
            let _ = writeln!(scroll_rules, "  {sel} {{");
            let mut inner = String::new();
            write_anims(&mut inner, &all, true);
            for line in inner.lines() {
                let _ = writeln!(scroll_rules, "  {line}");
            }
            scroll_rules.push_str("  }\n");
        }
        if !transitions.is_empty() {
            let _ = writeln!(
                out,
                "\n{sel} {{\n  transition: {};\n}}",
                transitions.join(", ")
            );
        }
        let hovered = format!("{sel}:hover,\n{sel}:focus-visible");
        if !hover_look.is_empty() {
            let _ = writeln!(out, "\n{hovered} {{\n  {}\n}}", hover_look.join("\n  "));
        }
        if !hover_motion.is_empty() {
            // Movement on hover is motion too: none for reduced motion.
            let _ = writeln!(
                out,
                "\n@media (prefers-reduced-motion: no-preference) {{\n  {} {{\n    {}\n  }}\n}}",
                hovered.replace('\n', "\n  "),
                hover_motion.join("\n    ")
            );
        }
        reduced.push(sel);
    }
    if !scroll_rules.is_empty() {
        let _ = write!(
            out,
            "\n@supports (animation-timeline: view()) {{\n{scroll_rules}}}\n"
        );
    }
    let _ = write!(
        out,
        "\n@media (prefers-reduced-motion: reduce) {{\n  {} {{\n    animation: none !important;\n    transition: none !important;\n  }}\n}}\n",
        reduced.join(",\n  ")
    );
    out
}

fn write_anims(out: &mut String, anims: &[Anim], timelines: bool) {
    let list: Vec<String> = anims
        .iter()
        .map(|a| format!("{} {}", a.name, a.timing))
        .collect();
    let _ = writeln!(out, "  animation: {};", list.join(", "));
    if timelines {
        let t: Vec<&str> = anims.iter().map(|a| a.timeline.as_str()).collect();
        let r: Vec<&str> = anims.iter().map(|a| a.range.as_str()).collect();
        let _ = writeln!(out, "  animation-timeline: {};", t.join(", "));
        let _ = writeln!(out, "  animation-range: {};", r.join(", "));
    }
    let c: Vec<&str> = anims.iter().map(|a| a.composition).collect();
    if c.iter().any(|c| *c != "replace") {
        let _ = writeln!(out, "  animation-composition: {};", c.join(", "));
    }
}
