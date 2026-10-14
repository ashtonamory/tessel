//! The agent link end to end (§5.4, §6.1, M4): an MCP client drives the
//! headless `DesignApp` through its real server, and every edit lands on
//! the UI thread as one transaction.

mod common;

use std::time::Duration;

use agent::DOCUMENT_URI;
use app::activity;
use common::mcp::Session;
use common::Harness;
use doc::{Command, Id, NodeId, Origin};
use egui::{Key, Modifiers};
use serde_json::{json, Value};

fn page_id(h: &Harness) -> String {
    h.ed().page().id.to_string()
}

fn node_id(v: &Value) -> NodeId {
    Id::from_raw(v.as_str().expect("an id"))
}

/// A 360×640 screen with a header and an auto-layout column whose height
/// hugs its two children.
fn screen(page: &str) -> Value {
    json!({ "parent_id": page, "node": {
        "name": "Screen", "kind": "frame", "x": 0, "y": 0, "width": 360, "height": 640,
        "fills": [{ "type": "solid", "color": "#ffffff" }],
        "children": [
            { "name": "Header", "kind": "frame", "width": 360, "height": 56 },
            { "name": "List", "kind": "frame", "x": 0, "y": 80, "width": 360, "height": 10,
              "layout": { "direction": "column", "gap": 8, "width": "fixed", "height": "hug" },
              "children": [
                { "name": "Row 1", "kind": "rect", "width": 360, "height": 40 },
                { "name": "Row 2", "kind": "rect", "width": 360, "height": 40 }
              ] }
        ] } })
}

fn create_screen(h: &mut Harness, a: &mut Session) -> Value {
    let args = screen(&page_id(h));
    a.ok(h, "create_node", args)
}

#[test]
fn an_agent_builds_a_screen_that_one_ctrl_z_removes() {
    let mut h = Harness::new();
    let before = h.ed().doc.clone();
    let mut a = Session::connect(&mut h, "claude-code");
    assert_eq!(
        h.app.agent_indicator().as_deref(),
        Some("1 agent: claude-code")
    );

    let r = create_screen(&mut h, &mut a);
    let id = node_id(&r["id"]);
    let ed = h.ed();
    let node = ed.node(&id).expect("the screen is in the document");
    assert_eq!(node.children().unwrap().len(), 2);
    let list = &node.children().unwrap()[1];
    assert_eq!(
        list.height, 88.0,
        "the hug height is written back: {list:?}"
    );

    // One transaction, labelled and tagged with the client.
    let txs = ed.history.transactions();
    assert_eq!(txs.len(), 1);
    assert_eq!(txs[0].label, "Agent: create Screen");
    assert_eq!(
        txs[0].origin,
        Origin::Agent {
            client: "claude-code".into()
        }
    );
    let rows = activity::entries(&ed.history, ed.clock.now());
    assert_eq!(rows[0].tag().as_deref(), Some("Agent: claude-code"));
    assert_eq!(app::app::undo_text(ed), "Undo Agent: create Screen");

    // Reads see the commit straight away.
    let got = a.ok(
        &mut h,
        "get_node",
        json!({ "id": id.to_string(), "depth": 2 }),
    );
    assert_eq!(got["node"]["children"][1]["height"], 88.0);
    assert_eq!(r["revision"], h.app.agent_host().unwrap().revision());

    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.ed().doc, before, "one Ctrl+Z removes the whole screen");
    let doc = a.ok(&mut h, "get_document", json!({}));
    assert!(doc["revision"].as_u64().unwrap() > r["revision"].as_u64().unwrap());
}

#[test]
fn selection_flows_both_ways() {
    let mut h = Harness::new();
    let mut a = Session::connect(&mut h, "claude-code");
    let r = create_screen(&mut h, &mut a);
    let id = node_id(&r["id"]);

    h.app.editor_mut().selection = vec![id.clone()];
    h.step(vec![]);
    let sel = a.ok(&mut h, "get_selection", json!({}));
    assert_eq!(sel["ids"], json!([id.to_string()]));

    h.app.editor_mut().selection.clear();
    let out = a.ok(
        &mut h,
        "set_selection",
        json!({ "ids": [id.to_string()], "zoom_to": true }),
    );
    assert_eq!(out["ids"], json!([id.to_string()]));
    assert_eq!(h.ed().selection, vec![id]);
}

#[test]
fn locked_nodes_and_read_only_mode_refuse_edits() {
    let mut h = Harness::new();
    let mut a = Session::connect(&mut h, "claude-code");
    let r = create_screen(&mut h, &mut a);
    let id = node_id(&r["id"]);

    h.app.editor_mut().set_locked(&id, true);
    h.step(vec![]);
    let e = a.err(
        &mut h,
        "update_node",
        json!({ "id": id.to_string(), "patch": { "x": 50 } }),
        "locked",
    );
    assert_eq!(e["node_id"], id.to_string());
    h.app.editor_mut().set_locked(&id, false);
    h.step(vec![]);

    h.app.agent_host().unwrap().set_agents_may_edit(false);
    assert!(h.app.agent_indicator().unwrap().ends_with("(read-only)"));
    a.err(
        &mut h,
        "update_node",
        json!({ "id": id.to_string(), "patch": { "x": 50 } }),
        "read_only",
    );
    a.ok(&mut h, "get_document", json!({}));
    h.app.agent_host().unwrap().set_agents_may_edit(true);
    a.ok(
        &mut h,
        "update_node",
        json!({ "id": id.to_string(), "patch": { "x": 50 } }),
    );
    assert_eq!(h.ed().node(&id).unwrap().x, 50.0);
}

#[test]
fn stale_revisions_are_refused() {
    let mut h = Harness::new();
    let mut a = Session::connect(&mut h, "claude-code");
    let r = create_screen(&mut h, &mut a);
    let id = node_id(&r["id"]);
    let rev = r["revision"].as_u64().unwrap();

    // The user edits in between.
    h.app.editor_mut().set_visible(&id, false);
    h.step(vec![]);
    a.err(
        &mut h,
        "update_node",
        json!({ "id": id.to_string(), "patch": { "x": 1 }, "if_revision": rev }),
        "stale_revision",
    );
    let now = h.app.agent_host().unwrap().revision();
    a.ok(
        &mut h,
        "update_node",
        json!({ "id": id.to_string(), "patch": { "x": 1 }, "if_revision": now }),
    );
}

/// The user starts dragging the node the agent wants to edit.
fn start_drag(h: &mut Harness, id: &NodeId) {
    let ed = h.app.editor_mut();
    ed.selection = vec![id.clone()];
    ed.begin_gesture("Move").unwrap();
    let n = ed.node(id).unwrap().clone();
    ed.gesture_apply(Command::SetGeometry {
        id: id.clone(),
        x: n.x + 10.0,
        y: n.y,
        width: n.width,
        height: n.height,
        rotation: n.rotation,
    })
    .unwrap();
}

#[test]
fn an_edit_waits_for_the_users_gesture_then_applies() {
    let mut h = Harness::new();
    h.app.editor_mut().clock = app::activity::Clock::manual();
    let mut a = Session::connect(&mut h, "claude-code");
    let r = create_screen(&mut h, &mut a);
    let id = node_id(&r["id"]);

    start_drag(&mut h, &id);
    let call = a.start(
        &h,
        "update_node",
        json!({ "id": id.to_string(), "patch": { "name": "Renamed" } }),
    );
    for _ in 0..20 {
        h.step(vec![]);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!call.is_finished(), "the edit waits for the drag");
    assert_eq!(h.app.agent_host().unwrap().waiting(), 1);

    h.app.editor_mut().end_gesture();
    let r = a.finish(&mut h, call);
    assert!(!r.is_error, "{}", r.json);
    let ed = h.ed();
    let node = ed.node(&id).unwrap();
    assert_eq!((node.name.as_str(), node.x), ("Renamed", 10.0));
    let labels: Vec<&str> = ed
        .history
        .transactions()
        .iter()
        .map(|t| t.label.as_str())
        .collect();
    assert_eq!(
        labels,
        ["Agent: create Screen", "Move", "Agent: update Screen"]
    );
}

#[test]
fn an_edit_gives_up_after_two_seconds_of_gesture() {
    let mut h = Harness::new();
    let clock = app::activity::Clock::manual();
    h.app.editor_mut().clock = clock.clone();
    let mut a = Session::connect(&mut h, "claude-code");
    let r = create_screen(&mut h, &mut a);
    let id = node_id(&r["id"]);
    let args = json!({ "parent_id": page_id(&h), "node": { "name": "Other", "kind": "rect",
        "x": 500, "y": 0, "width": 10, "height": 10 } });
    let other = node_id(&a.ok(&mut h, "create_node", args)["id"]);

    start_drag(&mut h, &id);
    // Nodes outside the gesture are edited at once.
    a.ok(
        &mut h,
        "update_node",
        json!({ "id": other.to_string(), "patch": { "x": 600 } }),
    );
    // The dragged node itself waits, and gives up after 2 s.
    let call = a.start(
        &h,
        "update_node",
        json!({ "id": id.to_string(), "patch": { "x": 99 } }),
    );
    h.step(vec![]);
    std::thread::sleep(Duration::from_millis(20));
    h.step(vec![]);
    assert!(!call.is_finished());
    clock.advance(Duration::from_millis(2100));
    let r = a.finish(&mut h, call);
    assert!(r.is_error);
    assert_eq!(r.json["code"], "locked");
    assert!(r.json["message"]
        .as_str()
        .unwrap()
        .contains("still editing"));
    assert!(h.ed().in_gesture(), "the user's drag is untouched");
    assert_eq!(h.ed().node(&id).unwrap().x, 10.0);
}

#[test]
fn subscribers_hear_about_commits() {
    let mut h = Harness::new();
    let mut a = Session::connect(&mut h, "claude-code");
    let mut b = Session::connect(&mut h, "watcher");
    assert_eq!(
        h.app.agent_indicator().as_deref(),
        Some("2 agents: claude-code, watcher")
    );
    b.subscribe(&mut h, DOCUMENT_URI);
    create_screen(&mut h, &mut a);
    let n = b
        .notification(&mut h, Duration::from_secs(3))
        .expect("a resource-updated notification");
    assert_eq!(n["method"], "notifications/resources/updated");
    assert_eq!(n["params"]["uri"], DOCUMENT_URI);
}

#[test]
fn disconnect_closes_a_session() {
    let mut h = Harness::new();
    let _a = Session::connect(&mut h, "claude-code");
    let host = h.app.agent_host().unwrap();
    let id = host.sessions()[0].id;
    host.disconnect(id);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !h.app.agent_host().unwrap().sessions().is_empty() {
        assert!(std::time::Instant::now() < deadline, "session still open");
        h.step(vec![]);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.app.agent_indicator().as_deref(), Some("No agents"));
}

#[test]
fn screenshots_without_a_gpu_report_an_internal_error() {
    let mut h = Harness::new();
    let mut a = Session::connect(&mut h, "claude-code");
    let r = create_screen(&mut h, &mut a);
    let e = a.err(&mut h, "screenshot", json!({ "id": r["id"] }), "internal");
    assert!(e["message"].as_str().unwrap().contains("wgpu"), "{e}");
    a.err(
        &mut h,
        "screenshot",
        json!({ "id": "n_missing0001" }),
        "not_found",
    );
}

#[test]
fn export_file_writes_svg_inside_the_document_folder_only() {
    let dir = std::env::temp_dir().join(format!("tessel-app-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = Harness::new();
    h.app.editor_mut().path = Some(dir.join("a.tessel.json"));
    let mut a = Session::connect(&mut h, "claude-code");
    let r = create_screen(&mut h, &mut a);
    let out = dir.join("out/screen.svg");
    let w = a.ok(
        &mut h,
        "export_file",
        json!({ "id": r["id"], "format": "svg", "path": out.to_str().unwrap() }),
    );
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.contains("<svg"), "{svg}");
    assert_eq!(w["bytes"].as_u64(), Some(svg.len() as u64), "{w}");
    a.err(
        &mut h,
        "export_file",
        json!({ "id": r["id"], "format": "svg", "path": "/etc/god-test.svg" }),
        "invalid_args",
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_window_listens_on_a_private_socket_and_cleans_up() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("tessel-app-sock-{}/tessel", std::process::id()));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    let mut h = Harness::new();
    let ctx = h.ctx.clone();
    h.app.start_agent(&ctx, Some(&dir));
    let sock = h.app.agent_host().unwrap().socket_path().expect("a socket");
    assert_eq!(sock, dir.join(format!("{}.sock", std::process::id())));
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&sock), 0o600);
    assert_eq!(
        std::fs::canonicalize(dir.join("current.sock")).unwrap(),
        std::fs::canonicalize(&sock).unwrap()
    );

    // A real connection over the socket.
    let host = h.app.agent_host().unwrap();
    let rt = host.runtime().clone();
    let stream = rt.block_on(tokio::net::UnixStream::connect(&sock)).unwrap();
    let task = common::mcp::spawn(&h, async move {
        let mut c = common::mcp::Client::new(stream);
        c.initialize("socket-client").await;
        c.call("get_document", json!({})).await
    });
    let r = task.finish(&mut h);
    assert!(!r.is_error, "{}", r.json);
    assert_eq!(r.json["pages"].as_array().unwrap().len(), 1);

    h.app.stop_agent();
    assert!(!sock.exists());
    assert!(std::fs::symlink_metadata(dir.join("current.sock")).is_err());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}
