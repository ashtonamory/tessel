//! Motion effects (§13.3): the file format, patches and validation.

use doc::*;
use serde_json::json;

fn rect() -> Node {
    Node::new(
        Id::from_raw("n_00000000r1"),
        "Card",
        NodeKind::Rect(Rect::default()),
    )
    .with_geometry(0.0, 0.0, 100.0, 50.0)
}

#[test]
fn motion_round_trips_and_is_omitted_when_empty() {
    let plain = serde_json::to_value(rect()).unwrap();
    assert!(plain.get("motion").is_none(), "{plain}");

    let v = json!({
        "id": "n_00000000r1", "name": "Card", "kind": "rect",
        "x": 0, "y": 0, "width": 100, "height": 50,
        "motion": [
            { "parallax": { "speed": 0.5 } },
            { "reveal": { "effect": "slide_up" } },
            { "hover": { "scale": 1.5, "lift": 4, "fill": "#2255ff" } },
            { "loop": { "effect": "float", "amount": 8 } }
        ]
    });
    let n: Node = serde_json::from_value(v).unwrap();
    assert_eq!(n.motion.len(), 4);
    assert_eq!(n.motion[0], Motion::Parallax { speed: 0.5 });
    assert_eq!(
        n.motion[1],
        Motion::Reveal {
            effect: RevealEffect::SlideUp,
            distance: 24.0
        }
    );
    match &n.motion[2] {
        Motion::Hover {
            scale,
            lift,
            opacity,
            duration_ms,
            ..
        } => {
            assert_eq!(
                (*scale, *lift, *opacity, *duration_ms),
                (1.5, 4.0, None, 150)
            );
        }
        other => panic!("{other:?}"),
    }
    let back = serde_json::to_value(&n).unwrap();
    assert_eq!(back["motion"][0], json!({ "parallax": { "speed": 0.5 } }));
    assert_eq!(
        back["motion"][2],
        json!({ "hover": { "scale": 1.5, "lift": 4.0, "fill": "#2255ff", "duration_ms": 150 } })
    );
    let again: Node = serde_json::from_value(back).unwrap();
    assert_eq!(again, n);
}

#[test]
fn unknown_effects_and_fields_are_refused() {
    let bad = json!({ "id": "n_00000000r1", "kind": "rect", "motion": [{ "wobble": {} }] });
    assert!(serde_json::from_value::<Node>(bad).is_err());
    let bad = json!({ "id": "n_00000000r1", "kind": "rect",
        "motion": [{ "parallax": { "speed": 1, "sped": 2 } }] });
    assert!(serde_json::from_value::<Node>(bad).is_err());
}

#[test]
fn a_patch_sets_and_undoes_motion() {
    let mut n = rect();
    let patch =
        NodePatch::from_json(json!({ "motion": [{ "parallax": { "speed": 0.3 } }] })).unwrap();
    let inverse = patch.apply_to(&mut n).unwrap();
    assert_eq!(n.motion, vec![Motion::Parallax { speed: 0.3 }]);
    inverse.apply_to(&mut n).unwrap();
    assert!(n.motion.is_empty());
}

fn problems_with(motion: serde_json::Value, kind: &str) -> Vec<String> {
    let text = json!({
        "format": { "name": "tessel", "version": 1 },
        "pages": [{ "id": "p_0000000001", "name": "Page", "children": [{
            "id": "n_00000000r1", "name": "N", "kind": kind,
            "width": 10, "height": 10, "motion": motion
        }]}]
    })
    .to_string();
    load_str(&text)
        .unwrap()
        .problems
        .into_iter()
        .map(|p| p.message)
        .collect()
}

#[test]
fn validation_checks_kinds_duplicates_and_ranges() {
    assert!(problems_with(json!([{ "parallax": { "speed": 0.5 } }]), "rect").is_empty());
    let p = problems_with(json!([{ "parallax": { "speed": 0.5 } }]), "group");
    assert!(
        p.iter().any(|m| m.contains("not allowed on a group")),
        "{p:?}"
    );
    let p = problems_with(
        json!([{ "parallax": { "speed": 0.5 } }, { "parallax": { "speed": 0.2 } }]),
        "rect",
    );
    assert!(
        p.iter().any(|m| m.contains("more than one parallax")),
        "{p:?}"
    );
    let p = problems_with(json!([{ "parallax": { "speed": 5 } }]), "rect");
    assert!(
        p.iter().any(|m| m.contains("speed 5 not in -1..=2")),
        "{p:?}"
    );
    let p = problems_with(
        json!([{ "loop": { "effect": "pulse", "amount": 0 } }]),
        "rect",
    );
    assert!(
        p.iter().any(|m| m.contains("amount 0 not in 0.1..=4")),
        "{p:?}"
    );
    let p = problems_with(json!([{ "hover": { "fill": "blue" } }]), "rect");
    assert!(!p.is_empty(), "a bad hover color is reported");
}
