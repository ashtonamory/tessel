//! M2 history and files, driven through the headless app: nudge merging
//! from the keyboard, undo labels, the activity list, autosave and
//! recovery, reload on external change, and SVG import.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use app::activity::{entries, Clock};
use app::app::{redo_text, undo_text};
use app::{DesignApp, Editor};
use common::Harness;
use doc::{Command, Id, IdKind, Node, NodeId, NodeKind, Origin, Rect as RectKind, Selection};
use egui::{Key, Modifiers};

fn temp(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-tmp")
        .join(format!("hf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rect_node(name: &str, x: f32) -> Node {
    Node::new(
        Id::new(IdKind::Node),
        name,
        NodeKind::Rect(RectKind::default()),
    )
    .with_geometry(x, 0.0, 50.0, 50.0)
}

fn add(ed: &mut Editor, n: Node) -> NodeId {
    let id = n.id.clone();
    let parent = ed.page_ref();
    ed.commit(
        "Add",
        vec![Command::CreateNode {
            parent,
            index: None,
            node: n,
        }],
        vec![id.clone()],
    )
    .unwrap();
    id
}

/// A harness whose editor has a manual clock and one selected rect.
fn harness_with(mut ed: Editor, clock: &Clock) -> Harness {
    ed.clock = clock.clone();
    Harness::with_app(eframe::Frame::_new_kittest(), DesignApp::new(ed))
}

fn x_of(h: &Harness, id: &NodeId) -> f32 {
    h.ed().node(id).unwrap().x
}

#[test]
fn arrow_nudges_merge_within_a_second_and_not_after() {
    let clock = Clock::manual();
    let mut ed = Editor::new();
    let id = add(&mut ed, rect_node("R", 100.0));
    let mut h = harness_with(ed, &clock);
    assert_eq!(h.ed().selection, vec![id.clone()]);
    let steps = h.ed().history.transactions().len();

    h.key(Key::ArrowRight, Modifiers::NONE);
    clock.advance(Duration::from_millis(400));
    h.key(Key::ArrowRight, Modifiers::NONE);
    clock.advance(Duration::from_millis(400));
    h.key(Key::ArrowRight, Modifiers::SHIFT);
    assert_eq!(x_of(&h, &id), 112.0);
    assert_eq!(h.ed().history.transactions().len(), steps + 1);
    assert_eq!(undo_text(h.ed()), "Undo Nudge layer");

    // More than a second later starts a new step.
    clock.advance(Duration::from_millis(1500));
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.ed().history.transactions().len(), steps + 2);

    // Ctrl+Z undoes only the last; the merged one goes in one more.
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(x_of(&h, &id), 112.0);
    assert_eq!(redo_text(h.ed()), "Redo Nudge layer");
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(x_of(&h, &id), 100.0);
}

#[test]
fn move_and_resize_labels_count_layers() {
    let mut h = Harness::new();
    for x in [0.0, 150.0] {
        h.key(Key::R, Modifiers::NONE);
        h.drag(h.at(x, 0.0), h.at(x + 100.0, 100.0));
    }
    assert_eq!(undo_text(h.ed()), "Undo Create rectangle");
    h.key(Key::A, Modifiers::COMMAND);
    h.key(Key::V, Modifiers::NONE);
    h.drag(h.at(50.0, 50.0), h.at(60.0, 60.0));
    assert_eq!(undo_text(h.ed()), "Undo Move 2 layers");
}

#[test]
fn activity_lists_recent_transactions_with_agent_tags() {
    let clock = Clock::manual();
    let mut ed = Editor::new();
    ed.clock = clock.clone();
    let id = add(&mut ed, rect_node("R", 0.0));
    clock.advance(Duration::from_secs(90));

    // An agent edit through the shared history.
    let sel = Selection {
        ids: vec![id.clone()],
    };
    ed.history
        .commit(
            &mut ed.doc,
            "Agent: create pricing section",
            Origin::Agent {
                client: "claude".into(),
            },
            vec![Command::DeleteNode { id: id.clone() }],
            sel.clone(),
            Selection::default(),
        )
        .unwrap();

    let rows = entries(&ed.history, clock.now());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].label, "Agent: create pricing section");
    assert_eq!(rows[0].tag().as_deref(), Some("Agent: claude"));
    assert_eq!(rows[1].label, "Add");
    assert_eq!(rows[1].tag(), None);
    assert_eq!(rows[1].age, Duration::from_secs(90));

    // The panel renders open and closed.
    let mut h = harness_with(ed, &clock);
    assert!(!h.app.activity().open);
    h.app.activity_mut().open = true;
    h.step(vec![]);
    h.step(vec![]);
}

#[test]
fn autosave_every_30s_then_cleared_by_a_save() {
    let root = temp("autosave");
    let src = root.join("a.tessel.json");
    let recovery = root.join("rec");
    let clock = Clock::manual();
    let mut ed = Editor::new();
    ed.save_as(&src).unwrap();
    let mut h = harness_with(ed, &clock);
    h.app.set_recovery_dir(Some(recovery.clone()));
    let file = app::autosave::recovery_path(&recovery, Some(&src));

    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(10.0, 10.0), h.at(80.0, 80.0));
    assert!(h.ed().is_dirty());
    clock.advance(Duration::from_secs(29));
    h.step(vec![]);
    assert!(!file.exists());
    clock.advance(Duration::from_secs(1));
    h.step(vec![]);
    assert!(file.exists(), "autosaved after 30 s");

    // Saving cleans up the recovery file.
    h.key(Key::S, Modifiers::COMMAND);
    assert!(!h.ed().is_dirty());
    assert!(!file.exists());
}

#[test]
fn recovery_is_offered_at_launch_and_can_be_restored_or_discarded() {
    let root = temp("recover");
    let src = root.join("a.tessel.json");
    let recovery = root.join("rec");
    let clock = Clock::manual();

    // A saved file, plus a newer autosave with one more rectangle.
    let mut ed = Editor::new();
    ed.save_as(&src).unwrap();
    add(&mut ed, rect_node("Unsaved", 5.0));
    let rec = app::autosave::write_recovery(&recovery, Some(&src), &ed).unwrap();
    let later = SystemTime::now() + Duration::from_secs(30);
    std::fs::File::options()
        .write(true)
        .open(&rec)
        .unwrap()
        .set_modified(later)
        .unwrap();

    let launch = |clock: &Clock| {
        let mut h = harness_with(Editor::open(&src).unwrap(), clock);
        h.app.set_recovery_dir(Some(recovery.clone()));
        h.app.check_recovery();
        h.step(vec![]);
        h
    };

    let mut h = launch(&clock);
    assert_eq!(h.app.recovery_offer().unwrap().file, rec);
    assert!(h.ed().page().children.is_empty());
    h.app.answer_recovery(true);
    h.step(vec![]);
    assert_eq!(h.ed().page().children.len(), 1);
    assert_eq!(h.ed().page().children[0].name, "Unsaved");
    assert!(h.ed().is_dirty(), "restored work is unsaved");
    assert_eq!(h.ed().path.as_deref(), Some(src.as_path()));
    assert!(rec.exists(), "kept until saved");
    h.key(Key::S, Modifiers::COMMAND);
    assert!(!rec.exists());
    assert_eq!(Editor::open(&src).unwrap().page().children.len(), 1);

    // Discard: the autosave is deleted and the file stays as it was.
    let mut ed = Editor::open(&src).unwrap();
    add(&mut ed, rect_node("Another", 9.0));
    let rec = app::autosave::write_recovery(&recovery, Some(&src), &ed).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&rec)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let mut h = launch(&clock);
    assert!(h.app.recovery_offer().is_some());
    h.app.answer_recovery(false);
    assert!(h.app.recovery_offer().is_none());
    assert!(!rec.exists());
    assert_eq!(h.ed().page().children.len(), 1);
    assert!(!h.ed().is_dirty());
}

fn write_with(path: &Path, names: &[&str]) {
    let mut ed = Editor::new();
    for (i, n) in names.iter().enumerate() {
        add(&mut ed, rect_node(n, i as f32 * 60.0));
    }
    let tmp = path.with_extension("tmp");
    doc::save_file(&tmp, &ed.doc).unwrap();
    std::fs::rename(&tmp, path).unwrap();
    // Distinct mtimes even on coarse file systems.
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(names.len() as u64 * 5))
        .unwrap();
}

#[test]
fn external_change_reloads_silently_and_keeps_the_selection() {
    let root = temp("reload");
    let src = root.join("a.tessel.json");
    let clock = Clock::manual();
    // Same page and node ids in both versions: keep what the file has.
    let mut ed = Editor::new();
    let keep = add(&mut ed, rect_node("Keep", 0.0));
    let gone = add(&mut ed, rect_node("Gone", 60.0));
    ed.save_as(&src).unwrap();
    let mut h = harness_with(Editor::open(&src).unwrap(), &clock);
    h.app.set_recovery_dir(None);
    h.key(Key::A, Modifiers::COMMAND);
    assert_eq!(h.ed().selection.len(), 2);

    // Our own save is not an external change.
    clock.advance(Duration::from_secs(2));
    h.step(vec![]);
    assert!(h
        .ed()
        .status
        .as_deref()
        .is_none_or(|s| !s.contains("Reloaded")));

    // Another program rewrites the file without the second node.
    let mut other = Editor::open(&src).unwrap();
    other
        .commit(
            "Delete",
            vec![Command::DeleteNode { id: gone.clone() }],
            Vec::new(),
        )
        .unwrap();
    doc::save_file(&src, &other.doc).unwrap();
    bump_mtime(&src);

    // Not before the next poll...
    clock.advance(Duration::from_millis(300));
    h.step(vec![]);
    assert_eq!(h.ed().page().children.len(), 2);
    // ...but then, with no prompt.
    clock.advance(Duration::from_secs(1));
    h.step(vec![]);
    assert_eq!(h.ed().page().children.len(), 1);
    assert!(!h.app.is_asking_to_reload());
    assert!(!h.ed().is_dirty());
    assert_eq!(h.ed().selection, vec![keep]);
    assert!(h.ed().status.as_deref().unwrap().contains("Reloaded"));
}

fn bump_mtime(path: &Path) {
    let m = std::fs::metadata(path).unwrap().modified().unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(m + Duration::from_secs(10))
        .unwrap();
}

#[test]
fn external_change_with_unsaved_edits_asks_first() {
    let root = temp("ask");
    let src = root.join("a.tessel.json");
    let clock = Clock::manual();
    write_with(&src, &["One"]);
    let mut h = harness_with(Editor::open(&src).unwrap(), &clock);
    h.app.set_recovery_dir(None);

    h.key(Key::R, Modifiers::NONE);
    h.drag(h.at(300.0, 300.0), h.at(380.0, 380.0));
    assert!(h.ed().is_dirty());
    write_with(&src, &["One", "Two", "Three"]);
    clock.advance(Duration::from_secs(1));
    h.step(vec![]);
    assert!(h.app.is_asking_to_reload());
    assert_eq!(h.ed().page().children.len(), 2, "nothing changed yet");

    // Keep mine: the edits stay, and it does not ask again for this version.
    h.app.answer_reload(false);
    clock.advance(Duration::from_secs(2));
    h.step(vec![]);
    assert!(!h.app.is_asking_to_reload());
    assert!(h.ed().is_dirty());
    assert_eq!(h.ed().page().children.len(), 2);

    // A further change asks again; Reload takes the file.
    write_with(&src, &["A", "B", "C", "D"]);
    clock.advance(Duration::from_secs(1));
    h.step(vec![]);
    assert!(h.app.is_asking_to_reload());
    h.app.answer_reload(true);
    assert_eq!(h.ed().page().children.len(), 4);
    assert!(!h.ed().is_dirty());
}

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <path id="tri" d="M0 0 L40 0 L20 30 Z" fill="#ff0000"/>
  <rect x="50" y="10" width="30" height="20" fill="blue" stroke="#000"/>
  <circle cx="20" cy="70" r="10"/>
  <text x="1" y="1">hi</text>
</svg>"##;

#[test]
fn svg_import_is_one_undo_step_and_skips_what_it_cannot_read() {
    let mut h = Harness::new();
    h.app.import_svg_text(SVG);
    let kids = h.children();
    assert_eq!(kids.len(), 3);
    assert!(kids.iter().all(|n| matches!(n.kind, NodeKind::Vector(_))));
    assert_eq!(kids[0].name, "tri");
    assert_eq!(h.ed().selection.len(), 3);
    let status = h.ed().status.clone().unwrap();
    assert!(
        status.contains("Imported 3 shapes") && status.contains("<text>"),
        "{status}"
    );
    let NodeKind::Vector(v) = &kids[1].kind else {
        unreachable!()
    };
    assert_eq!(v.shape.fills.len(), 1);
    assert_eq!(v.shape.strokes.len(), 1);
    assert_eq!((kids[1].width, kids[1].height), (30.0, 20.0));
    assert!(v.path.starts_with("M0,0"), "{}", v.path);

    assert_eq!(undo_text(h.ed()), "Undo Import SVG");
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(h.children().is_empty());
    h.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.children().len(), 3);

    // Not an SVG: an error in the status bar, no change.
    h.app.import_svg_text("nope");
    assert_eq!(h.children().len(), 3);
}

fn image_node(ed: &Editor, bytes: &[u8]) -> Node {
    let asset = ed.assets.add(bytes.to_vec(), "png");
    Node::new(
        Id::new(IdKind::Node),
        "Pic",
        NodeKind::Image(doc::Image {
            asset,
            fit: doc::ImageFit::Fill,
            corner_radius: Default::default(),
            extra: Default::default(),
        }),
    )
    .with_geometry(0.0, 0.0, 20.0, 20.0)
}

#[test]
fn recovery_carries_images_added_since_the_last_save() {
    let root = temp("recovery-assets");
    let src = root.join("a.tessel.json");
    let recovery = root.join("rec");
    let mut ed = Editor::new();
    ed.save_as(&src).unwrap();
    let live_dir = doc::assets::assets_dir_for(&src);

    let bytes = b"not really a png, but bytes".to_vec();
    let n = image_node(&ed, &bytes);
    let asset = match &n.kind {
        NodeKind::Image(i) => i.asset.clone(),
        _ => unreachable!(),
    };
    add(&mut ed, n);
    assert!(!live_dir.exists());
    let rec = app::autosave::write_recovery(&recovery, Some(&src), &ed).unwrap();
    let rec_assets = doc::assets::assets_dir_for(&rec);
    assert_eq!(std::fs::read(rec_assets.join(&asset.0)).unwrap(), bytes);
    // The live store still reads from, and saves to, the document's own folder.
    assert_eq!(ed.assets.dir(), Some(live_dir.clone()));
    assert!(!live_dir.exists());

    // A fresh editor restores the image, and a save writes it beside the file.
    let mut fresh = app::autosave::restore(&rec, Some(&src)).unwrap();
    assert_eq!(fresh.assets.bytes(&asset).as_deref(), Some(&bytes));
    fresh.save_as(&src).unwrap();
    assert_eq!(std::fs::read(live_dir.join(&asset.0)).unwrap(), bytes);

    // Removing the recovery file removes its assets too.
    app::autosave::remove_recovery(&recovery, Some(&src));
    assert!(!rec.exists() && !rec_assets.exists());
}

#[test]
fn undo_after_a_save_is_unsaved_again_and_restores_the_earlier_state() {
    let root = temp("undo-save");
    let src = root.join("a.tessel.json");
    let mut ed = Editor::new();
    add(&mut ed, rect_node("One", 0.0));
    ed.save_as(&src).unwrap();
    assert!(!ed.is_dirty());
    let saved = ed.doc.clone();
    add(&mut ed, rect_node("Two", 60.0));
    ed.save_as(&src).unwrap();
    assert!(!ed.is_dirty());

    ed.undo();
    assert!(ed.is_dirty(), "the file still has the undone edit");
    assert_eq!(ed.doc, saved);
}

#[test]
fn asset_reads_reject_symlinks_oversized_files_and_unreferenced_names() {
    use doc::assets::MAX_ASSET_BYTES;
    use std::os::unix::fs::symlink;

    let root = temp("asset-reads");
    let dir = root.join("x.assets");
    std::fs::create_dir_all(&dir).unwrap();
    let outside = root.join("secret.bin");
    std::fs::write(&outside, b"secret").unwrap();
    std::fs::write(dir.join("aaaaaaaaaaaa.png"), b"good").unwrap();
    symlink(&outside, dir.join("bbbbbbbbbbbb.png")).unwrap();
    let big = std::fs::File::create(dir.join("cccccccccccc.png")).unwrap();
    big.set_len(MAX_ASSET_BYTES + 1).unwrap();
    std::fs::write(dir.join("dddddddddddd.png"), b"unreferenced").unwrap();

    let mut ed = Editor::new();
    let refs: Vec<_> = ["aaaaaaaaaaaa", "bbbbbbbbbbbb", "cccccccccccc"]
        .iter()
        .map(|h| doc::AssetRef(format!("{h}.png")))
        .collect();
    for r in &refs {
        let mut n = image_node(&ed, b"x");
        n.kind = NodeKind::Image(doc::Image {
            asset: r.clone(),
            fit: doc::ImageFit::Fill,
            corner_radius: Default::default(),
            extra: Default::default(),
        });
        add(&mut ed, n);
    }

    // load_dir takes only the good, referenced file.
    let a = doc::Assets::new();
    a.load_dir(&dir, ed.document());
    a.set_dir(None); // anything that is not in memory now cannot be read lazily
    assert_eq!(a.bytes(&refs[0]).as_deref(), Some(&b"good".to_vec()));
    assert!(a.bytes(&refs[1]).is_none());
    assert!(a.bytes(&refs[2]).is_none());
    assert!(a.bytes(&doc::AssetRef("dddddddddddd.png".into())).is_none());

    // The lazy read applies the same rules.
    let b = doc::Assets::new();
    b.set_dir(Some(dir.clone()));
    assert!(b.bytes(&refs[0]).is_some());
    assert!(b.bytes(&refs[1]).is_none(), "symlink");
    assert!(b.bytes(&refs[2]).is_none(), "oversized");
}
