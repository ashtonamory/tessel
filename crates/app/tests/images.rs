//! Placing images through the editor: one undoable command, assets stored by
//! content hash and saved next to the document.

use std::fs;
use std::path::PathBuf;

use app::place::encode_png;
use app::Editor;
use doc::NodeKind;
use kurbo::Rect;

fn png(w: u32, h: u32) -> Vec<u8> {
    encode_png(w, h, vec![200; (w * h * 4) as usize]).unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-tmp")
        .join(format!("app-images-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn image_count(ed: &Editor) -> usize {
    ed.page()
        .children
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Image(_)))
        .count()
}

#[test]
fn insert_places_at_natural_size_centered_and_undoes() {
    let mut ed = Editor::new();
    let view = Rect::new(0.0, 0.0, 1000.0, 800.0);
    let id = ed.insert_image(png(100, 50), "png", "logo", view).unwrap();
    let n = ed.node(&id).unwrap();
    assert_eq!(n.name, "logo");
    assert_eq!((n.x, n.y, n.width, n.height), (450.0, 375.0, 100.0, 50.0));
    assert_eq!(ed.selection, vec![id.clone()]);
    assert!(ed.is_dirty());

    ed.undo();
    assert_eq!(image_count(&ed), 0);
    assert!(ed.selection.is_empty());
    ed.redo();
    assert_eq!(image_count(&ed), 1);
    assert_eq!(ed.selection, vec![id]);
}

#[test]
fn large_images_are_scaled_down_to_the_viewport() {
    let mut ed = Editor::new();
    let view = Rect::new(0.0, 0.0, 200.0, 200.0);
    let id = ed.insert_image(png(400, 100), "png", "wide", view).unwrap();
    let n = ed.node(&id).unwrap();
    assert_eq!((n.width, n.height), (200.0, 50.0));
    assert_eq!((n.x, n.y), (0.0, 75.0));
}

#[test]
fn bad_bytes_are_rejected_without_changing_the_document() {
    let mut ed = Editor::new();
    let view = Rect::new(0.0, 0.0, 200.0, 200.0);
    assert!(ed.insert_image(b"nope".to_vec(), "png", "x", view).is_err());
    assert_eq!(image_count(&ed), 0);
    assert!(!ed.is_dirty());
}

#[test]
fn same_file_twice_is_stored_once_and_survives_save_and_open() {
    let dir = tmp("save");
    let file = dir.join("landing.tessel.json");
    let mut ed = Editor::new();
    let view = Rect::new(0.0, 0.0, 500.0, 500.0);
    let bytes = png(20, 20);
    let a = ed.insert_image(bytes.clone(), "png", "a", view).unwrap();
    let b = ed.insert_image(bytes.clone(), "png", "b", view).unwrap();
    ed.insert_image(png(30, 30), "png", "c", view).unwrap();
    ed.save_as(&file).unwrap();

    let assets = dir.join("landing.assets");
    assert_eq!(fs::read_dir(&assets).unwrap().count(), 2);

    let reopened = Editor::open(&file).unwrap();
    assert!(reopened.problems.is_empty());
    let asset_of = |ed: &Editor, id| match &ed.node(id).unwrap().kind {
        NodeKind::Image(i) => i.asset.clone(),
        _ => unreachable!(),
    };
    let r = asset_of(&reopened, &a);
    assert_eq!(r, asset_of(&reopened, &b));
    assert_eq!(reopened.assets.bytes(&r).unwrap().as_slice(), &bytes[..]);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn deleting_the_image_prunes_its_asset_on_save() {
    let dir = tmp("prune");
    let file = dir.join("t.tessel.json");
    let mut ed = Editor::new();
    let id = ed
        .insert_image(png(8, 8), "png", "a", Rect::new(0.0, 0.0, 100.0, 100.0))
        .unwrap();
    ed.save_as(&file).unwrap();
    assert!(dir.join("t.assets").is_dir());
    ed.selection = vec![id];
    ed.delete_selection();
    ed.save_as(&file).unwrap();
    assert!(!dir.join("t.assets").exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_asset_file_still_opens() {
    let dir = tmp("missing");
    let file = dir.join("t.tessel.json");
    let mut ed = Editor::new();
    let id = ed
        .insert_image(png(8, 8), "png", "a", Rect::new(0.0, 0.0, 100.0, 100.0))
        .unwrap();
    ed.save_as(&file).unwrap();
    fs::remove_dir_all(dir.join("t.assets")).unwrap();

    let reopened = Editor::open(&file).unwrap();
    let NodeKind::Image(i) = &reopened.node(&id).unwrap().kind else {
        unreachable!()
    };
    assert!(reopened.assets.bytes(&i.asset).is_none());
    // Saving again must not fail or invent the file.
    let mut reopened = reopened;
    reopened.save_as(&file).unwrap();
    fs::remove_dir_all(&dir).unwrap();
}
