//! `tessel mcp --headless`: the document tools, the folder it may touch, and
//! edits saved to the file at once. Rendering is covered in `render.rs`.

use std::path::{Path, PathBuf};

use agent::tools::{self, ToolOutput};
use headless::Workspace;
use serde_json::{json, Value};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tessel-headless-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn call(ws: &Workspace, name: &str, args: Value) -> Result<Value, String> {
    match tools::call(ws, "test", false, name, args)
        .await
        .expect("a known tool")
    {
        Ok(ToolOutput::Json(v)) => Ok(v),
        Ok(ToolOutput::Image { info, .. }) => Ok(info),
        Err(e) => Err(e.to_json()),
    }
}

fn png_file(path: &Path, width: u32, height: u32) {
    image::RgbaImage::from_pixel(width, height, image::Rgba([200, 30, 30, 255]))
        .save(path)
        .unwrap();
}

#[tokio::test]
async fn offers_the_document_tools_only_headless() {
    let names = |documents| {
        tools::list_for(documents)
            .into_iter()
            .map(|t| t.name.to_string())
            .collect::<Vec<_>>()
    };
    assert!(names(true).contains(&"new_document".to_string()));
    assert!(
        !names(false).contains(&"new_document".to_string()),
        "the app keeps its tool list"
    );
    assert_eq!(
        names(false),
        tools::list()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn new_document_makes_a_design_frame_and_saves_each_edit() {
    let root = temp_dir("new");
    let ws = Workspace::new(&root, &[]).unwrap();

    let refused = call(
        &ws,
        "create_node",
        json!({ "parent_id": "p_none000001", "node": { "kind": "rect" } }),
    )
    .await;
    assert!(refused.is_err(), "nothing is open yet");

    let made = call(
        &ws,
        "new_document",
        json!({ "path": "acme/sale.tessel.json", "width": 1080, "height": 1350 }),
    )
    .await
    .unwrap();
    assert_eq!(made["path"], "acme/sale.tessel.json");
    assert_eq!(made["title"], "sale");
    let frame = made["pages"][0]["children"][0].clone();
    assert_eq!(
        (frame["name"].clone(), frame["width"].clone()),
        (json!("Design"), json!(1080.0))
    );

    let again = call(
        &ws,
        "new_document",
        json!({ "path": "acme/sale.tessel.json", "width": 10, "height": 10 }),
    )
    .await;
    assert!(again.unwrap_err().contains("already exists"));

    let id = frame["id"].as_str().unwrap();
    call(&ws, "create_node", json!({ "parent_id": id, "node": { "kind": "text", "name": "Headline", "content": "Fall sale" } }))
        .await
        .unwrap();
    let saved = std::fs::read_to_string(root.join("acme/sale.tessel.json")).unwrap();
    assert!(
        saved.contains("Fall sale"),
        "the edit is on disk without a save call"
    );

    let listed = call(&ws, "list_documents", json!({})).await.unwrap();
    assert_eq!(
        listed["documents"],
        json!([{ "path": "acme/sale.tessel.json", "title": "sale" }])
    );
}

#[tokio::test]
async fn open_document_switches_files_and_stale_revisions_fail() {
    let root = temp_dir("open");
    let ws = Workspace::new(&root, &[]).unwrap();
    let a = call(
        &ws,
        "new_document",
        json!({ "path": "a.tessel.json", "width": 100, "height": 100 }),
    )
    .await
    .unwrap();
    call(
        &ws,
        "new_document",
        json!({ "path": "b.tessel.json", "width": 200, "height": 200 }),
    )
    .await
    .unwrap();
    let opened = call(&ws, "open_document", json!({ "path": "a.tessel.json" }))
        .await
        .unwrap();
    assert_eq!(
        opened["pages"][0]["children"][0]["id"],
        a["pages"][0]["children"][0]["id"]
    );
    let id = a["pages"][0]["children"][0]["id"].as_str().unwrap();
    let stale = call(
        &ws,
        "update_node",
        json!({ "id": id, "patch": { "name": "Old" }, "if_revision": a["revision"] }),
    )
    .await;
    assert!(
        stale.unwrap_err().contains("stale_revision"),
        "a revision from before the switch fails"
    );
}

#[tokio::test]
async fn stays_inside_its_folder() {
    let root = temp_dir("inside");
    let outside = temp_dir("outside");
    png_file(&outside.join("secret.png"), 4, 4);
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let ws = Workspace::new(&root, &[]).unwrap();

    for path in [
        "../x.tessel.json",
        "/tmp/x.tessel.json",
        ".hidden/x.tessel.json",
        "link/x.tessel.json",
        "x.json",
    ] {
        let r = call(
            &ws,
            "new_document",
            json!({ "path": path, "width": 10, "height": 10 }),
        )
        .await;
        assert!(r.is_err(), "{path} was allowed");
    }
    assert!(
        !outside.join("x.tessel.json").exists(),
        "nothing was written through the symlink"
    );

    call(
        &ws,
        "new_document",
        json!({ "path": "d.tessel.json", "width": 10, "height": 10 }),
    )
    .await
    .unwrap();
    let image = outside.join("secret.png");
    let r = call(
        &ws,
        "import_image",
        json!({ "path": image.to_str().unwrap() }),
    )
    .await;
    assert!(r.unwrap_err().contains("outside"));
    let id = call(&ws, "get_document", json!({})).await.unwrap()["pages"][0]["children"][0]["id"]
        .clone();
    let r = call(
        &ws,
        "export_file",
        json!({ "id": id, "format": "svg", "path": outside.join("out.svg").to_str().unwrap() }),
    )
    .await;
    assert!(r.is_err() && !outside.join("out.svg").exists());
}

#[tokio::test]
async fn imported_images_are_saved_once_a_layer_uses_them() {
    let root = temp_dir("images");
    let photos = temp_dir("photos");
    png_file(&photos.join("shop.png"), 30, 20);
    let ws = Workspace::new(&root, std::slice::from_ref(&photos)).unwrap();
    let made = call(
        &ws,
        "new_document",
        json!({ "path": "post.tessel.json", "width": 100, "height": 100 }),
    )
    .await
    .unwrap();
    let got = call(
        &ws,
        "import_image",
        json!({ "path": photos.join("shop.png").to_str().unwrap() }),
    )
    .await
    .unwrap();
    assert_eq!(
        (got["width"].clone(), got["height"].clone()),
        (json!(30), json!(20))
    );
    let asset = got["asset"].as_str().unwrap();
    let frame = made["pages"][0]["children"][0]["id"].as_str().unwrap();
    call(&ws, "create_node", json!({ "parent_id": frame, "node": { "kind": "image", "asset": asset, "width": 30, "height": 20 } }))
        .await
        .unwrap();
    assert!(root.join("post.assets").join(asset).is_file());
}
