//! Content-addressed assets: dedupe, save/load round trip, pruning (§3.8).

use std::fs;
use std::path::PathBuf;

use doc::assets::{assets_dir_for, referenced};
use doc::{
    load_file, save_file, Assets, Document, Id, IdKind, Image, ImageFit, Node, NodeKind, Page,
};

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-tmp")
        .join(format!("assets-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn doc_with(assets: &[doc::AssetRef]) -> Document {
    let mut page = Page::new(Id::from_entropy(IdKind::Page, 1), "Page");
    for (i, a) in assets.iter().enumerate() {
        page.children.push(
            Node::new(
                Id::from_entropy(IdKind::Node, i as u64 + 1),
                "img",
                NodeKind::Image(Image {
                    asset: a.clone(),
                    fit: ImageFit::Fill,
                    corner_radius: Default::default(),
                    extra: Default::default(),
                }),
            )
            .with_geometry(0.0, 0.0, 10.0, 10.0),
        );
    }
    Document::new("t", page)
}

#[test]
fn same_content_is_stored_once_and_named_by_hash() {
    let assets = Assets::new();
    let a = assets.add(b"hello".to_vec(), "png");
    let b = assets.add(b"hello".to_vec(), "PNG");
    let c = assets.add(b"other".to_vec(), "png");
    assert_eq!(a, b);
    assert_ne!(a, c);
    // sha256("hello") = 2cf24dba5fb0a30e…
    assert_eq!(a.0, "2cf24dba5fb0.png");
    assert_eq!(assets.bytes(&a).unwrap().as_slice(), b"hello");
}

#[test]
fn save_writes_assets_and_load_reads_them_back() {
    let dir = tmp("roundtrip");
    let file = dir.join("landing.tessel.json");
    let assets = Assets::new();
    let a = assets.add(b"image bytes".to_vec(), "png");
    let d = doc_with(&[a.clone(), a.clone()]);
    save_file(&file, &d).unwrap();
    assets.save(&assets_dir_for(&file), &d).unwrap();
    // Two nodes, one file.
    let dir_assets = dir.join("landing.assets");
    assert_eq!(fs::read_dir(&dir_assets).unwrap().count(), 1);
    assert_eq!(fs::read(dir_assets.join(&a.0)).unwrap(), b"image bytes");

    let loaded = load_file(&file).unwrap();
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    assert_eq!(referenced(&loaded.doc), [a.clone()].into());
    let fresh = Assets::new();
    fresh.set_dir(Some(assets_dir_for(&file)));
    assert_eq!(fresh.bytes(&a).unwrap().as_slice(), b"image bytes");
    assert!(fresh.bytes(&doc::AssetRef("../x.png".into())).is_none());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unreferenced_assets_are_pruned_and_empty_dir_removed() {
    let dir = tmp("prune");
    let file = dir.join("t.tessel.json");
    let assets = Assets::new();
    let a = assets.add(b"one".to_vec(), "png");
    let b = assets.add(b"two".to_vec(), "jpg");
    let adir = assets_dir_for(&file);
    assets
        .save(&adir, &doc_with(&[a.clone(), b.clone()]))
        .unwrap();
    fs::write(adir.join("notes.txt"), "keep me").unwrap();
    assets
        .save(&adir, &doc_with(std::slice::from_ref(&a)))
        .unwrap();
    assert!(adir.join(&a.0).exists());
    assert!(!adir.join(&b.0).exists());
    assert!(adir.join("notes.txt").exists());
    fs::remove_file(adir.join("notes.txt")).unwrap();
    assets.save(&adir, &doc_with(&[])).unwrap();
    assert!(!adir.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pruned_asset_survives_undo() {
    let dir = tmp("undo");
    let file = dir.join("t.tessel.json");
    let adir = assets_dir_for(&file);
    let r = doc::AssetRef("2cf24dba5fb0.png".into());
    fs::create_dir_all(&adir).unwrap();
    fs::write(adir.join(&r.0), b"hello").unwrap();
    let with = doc_with(std::slice::from_ref(&r));
    save_file(&file, &with).unwrap();

    // Loaded, but the asset bytes are never read.
    let loaded = load_file(&file).unwrap();
    let assets = Assets::new();
    assets.set_dir(Some(adir.clone()));
    let without = doc_with(&[]);
    assets.save(&adir, &without).unwrap();
    assert!(!adir.join(&r.0).exists());

    // Undo restores the node; the bytes must still be available and re-saved.
    assert_eq!(assets.bytes(&r).unwrap().as_slice(), b"hello");
    assets.save(&adir, &loaded.doc).unwrap();
    assert_eq!(fs::read(adir.join(&r.0)).unwrap(), b"hello");
    fs::remove_dir_all(&dir).unwrap();
}
