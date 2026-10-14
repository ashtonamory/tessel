//! MCP protocol tests: a raw JSON-RPC client drives the server over an
//! in-memory pipe against `FakeApp`.

mod common;

use std::time::Duration;

use base64::Engine as _;
use common::*;
use doc::{Command, NodeKind, NodePatch, Origin, Paint};
use serde_json::{json, Value};

fn agent_origin(client: &str) -> Origin {
    Origin::Agent {
        client: client.into(),
    }
}

#[tokio::test]
async fn initialize_reports_server_and_capabilities() {
    let agent = agent::Agent::new(FakeApp::new(fixture(), None));
    let mut c = connect(&agent);
    let init = c.initialize("claude-code").await;
    assert_eq!(init["serverInfo"]["name"], "tessel");
    assert_eq!(init["protocolVersion"], "2025-06-18");
    let caps = &init["capabilities"];
    assert!(caps["tools"].is_object(), "{caps}");
    assert!(caps["prompts"].is_object(), "{caps}");
    assert_eq!(caps["resources"]["subscribe"], true, "{caps}");
    assert!(init["instructions"]
        .as_str()
        .unwrap()
        .contains("get_selection"));
    // The session is known by the client name from `initialize`.
    let sessions = agent.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].client, "claude-code");
}

#[tokio::test]
async fn newer_protocol_versions_negotiate_down_to_one_with_initialize() {
    let agent = agent::Agent::new(FakeApp::new(fixture(), None));
    let mut c = connect(&agent);
    let init = c
        .result(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "x", "version": "1" }
            }),
        )
        .await;
    assert_eq!(init["protocolVersion"], "2025-11-25");
}

#[tokio::test]
async fn tools_list_has_every_m4_tool_with_object_schemas() {
    let (_agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let list = c.result("tools/list", json!({})).await;
    let names: Vec<&str> = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "get_document",
            "get_node",
            "find_nodes",
            "get_selection",
            "get_tokens",
            "create_node",
            "update_node",
            "update_nodes",
            "delete_nodes",
            "move_node",
            "duplicate_node",
            "set_tokens",
            "upsert_style",
            "apply_style",
            "create_component",
            "create_instance",
            "set_selection",
            "screenshot",
            "export_code",
            "export_file",
        ]
    );
    for t in list["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
        assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
    }
}

#[tokio::test]
async fn unknown_tool_is_a_protocol_error_and_bad_args_are_invalid_args() {
    let (_agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let resp = c
        .request("tools/call", json!({ "name": "nope", "arguments": {} }))
        .await;
    assert_eq!(resp["error"]["code"], -32602, "{resp}");
    // Missing a required argument.
    c.err("get_node", json!({}), "invalid_args").await;
    // Unknown argument (typo).
    c.err(
        "get_node",
        json!({ "id": SCREEN, "depht": 2 }),
        "invalid_args",
    )
    .await;
}

// -- Read tools -------------------------------------------------------------

#[tokio::test]
async fn get_document_summarises_below_depth() {
    let (_agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let d = c.ok("get_document", json!({})).await;
    assert_eq!(d["revision"], 0);
    assert_eq!(d["title"], "Fixture");
    assert_eq!(d["dirty"], false);
    assert_eq!(d["path"], Value::Null);
    let pages = d["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 2);
    let screen = &pages[0]["children"][0];
    assert_eq!(screen["id"], SCREEN);
    assert_eq!(screen["kind"], "frame");
    assert_eq!(screen["width"], 400.0);
    // Depth 1: top-level nodes, their children counted.
    assert_eq!(screen["children"], 3);
    assert_eq!(d["tokens"]["colors"]["color.primary"], "#2563eb");
    assert_eq!(
        d["styles"],
        json!([{ "id": FILL_STYLE, "name": "Surface/Card", "kind": "fill" }])
    );
    assert_eq!(d["components"], json!([]));

    let d = c
        .ok("get_document", json!({ "page_id": PAGE, "depth": 2 }))
        .await;
    assert_eq!(d["pages"].as_array().unwrap().len(), 1);
    let kids = &d["pages"][0]["children"][0]["children"];
    assert_eq!(kids[0]["name"], "Bg");
    assert_eq!(kids[2]["children"], 1, "group's child is counted");

    c.err(
        "get_document",
        json!({ "page_id": "p_zzzzzzzzzz" }),
        "not_found",
    )
    .await;
}

#[tokio::test]
async fn get_node_returns_file_json_geometry_and_path() {
    let (_agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let n = c.ok("get_node", json!({ "id": BG })).await;
    assert_eq!(n["node"]["id"], BG);
    assert_eq!(n["node"]["kind"], "rect");
    assert_eq!(n["node"]["x"], 10.0);
    assert_eq!(n["parent_id"], SCREEN);
    assert_eq!(n["index"], 0);
    assert_eq!(n["path"], json!(["Home", "Screen", "Bg"]));
    assert_eq!(
        n["geometry"],
        json!({ "x": 10.0, "y": 20.0, "width": 100.0, "height": 50.0, "rotation": 0.0 })
    );

    // Depth 0 counts the children; depth 1 lists them in full with their
    // own children counted.
    let n = c
        .ok(
            "get_node",
            json!({ "id": SCREEN, "depth": 0, "include_geometry": false }),
        )
        .await;
    assert_eq!(n["node"]["children"], 3);
    assert!(n.get("geometry").is_none());
    let n = c.ok("get_node", json!({ "id": SCREEN })).await;
    assert_eq!(n["node"]["children"][1]["content"], "Hello world");
    assert_eq!(n["node"]["children"][2]["children"], 1);
    assert_eq!(n["parent_id"], PAGE);

    let e = c
        .err("get_node", json!({ "id": "n_zzzzzzzzzz" }), "not_found")
        .await;
    assert_eq!(e["node_id"], "n_zzzzzzzzzz");
}

#[tokio::test]
async fn find_nodes_by_name_kind_text_and_within() {
    let (_agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let ids = |v: &Value| -> Vec<String> {
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect()
    };
    let r = c.ok("find_nodes", json!({ "name": "TITLE" })).await;
    assert_eq!(ids(&r), [TITLE]);
    let r = c.ok("find_nodes", json!({ "kind": "rect" })).await;
    assert_eq!(ids(&r), [BG, LOCKED_CHILD]);
    let r = c.ok("find_nodes", json!({ "text": "hello" })).await;
    assert_eq!(ids(&r), [TITLE]);
    let r = c
        .ok("find_nodes", json!({ "kind": "rect", "within": LOCKED }))
        .await;
    assert_eq!(ids(&r), [LOCKED_CHILD]);
    let r = c.ok("find_nodes", json!({ "limit": 2 })).await;
    assert_eq!(ids(&r).len(), 2);
    c.err(
        "find_nodes",
        json!({ "within": "n_zzzzzzzzzz" }),
        "not_found",
    )
    .await;
}

#[tokio::test]
async fn get_selection_and_get_tokens() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let s = c.ok("get_selection", json!({})).await;
    assert_eq!(s["ids"], json!([]));
    assert!(s.get("focused_frame_id").is_none());
    agent.app().user_select(&[BG, TITLE]);
    let s = c.ok("get_selection", json!({})).await;
    assert_eq!(s["page_id"], PAGE);
    assert_eq!(s["ids"], json!([BG, TITLE]));
    assert_eq!(s["nodes"][0]["name"], "Bg");
    assert_eq!(s["focused_frame_id"], SCREEN);

    let t = c.ok("get_tokens", json!({})).await;
    assert_eq!(t["tokens"]["colors"]["color.primary"], "#2563eb");
    assert_eq!(t["styles"][0]["fills"][0]["color"], "#ffffff");
}

// -- Write tools ------------------------------------------------------------

#[tokio::test]
async fn create_node_builds_a_subtree_in_one_undo_step() {
    let app = FakeApp::new(fixture(), None);
    let before = app.doc();
    let (agent, mut c) = start(app, "claude-code").await;
    let r = c
        .ok(
            "create_node",
            json!({
                "parent_id": PAGE,
                "select": true,
                "node": {
                    "name": "Card", "kind": "frame", "x": 600, "y": 0,
                    "width": 320, "height": 200,
                    "fills": [{ "type": "solid", "color": "{color.primary}" }],
                    "layout": { "direction": "column", "gap": 8, "padding": [16, 16, 16, 16] },
                    "children": [
                        { "name": "Title", "kind": "text", "content": "Pro" },
                        { "name": "Item", "kind": "text", "content": "One" },
                        { "name": "Item", "kind": "text", "content": "Two" },
                        { "name": "Row", "kind": "frame", "children": [
                            { "kind": "rect", "width": 10, "height": 10 }
                        ]}
                    ]
                }
            }),
        )
        .await;
    assert_eq!(r["revision"], 1);
    let ids = r["ids"].as_object().unwrap();
    let keys: Vec<&str> = ids.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "Card",
            "Card/Item",
            "Card/Item#2",
            "Card/Row",
            "Card/Row/rect",
            "Card/Title"
        ]
    );
    assert_eq!(r["id"], ids["Card"]);
    let doc = agent.app().doc();
    let card = doc.node(&id(r["id"].as_str().unwrap())).unwrap();
    assert_eq!(card.children().unwrap().len(), 4);
    let title = doc.node(&id(ids["Card/Title"].as_str().unwrap())).unwrap();
    assert!(matches!(&title.kind, NodeKind::Text(t) if t.content == "Pro"));
    assert_eq!(agent.app().selection(), [id(r["id"].as_str().unwrap())]);
    assert_eq!(
        agent.app().history(),
        [(
            "Agent: create Card".to_string(),
            agent_origin("claude-code")
        )]
    );
    // One Ctrl+Z removes the whole subtree.
    agent.app().undo();
    assert_eq!(agent.app().doc(), before);

    // Into a node, at an index.
    let r = c
        .ok(
            "create_node",
            json!({ "parent_id": SCREEN, "index": 0, "node": { "name": "Under", "kind": "rect" } }),
        )
        .await;
    let doc = agent.app().doc();
    assert_eq!(
        doc.node(&id(SCREEN)).unwrap().children().unwrap()[0].id,
        id(r["id"].as_str().unwrap())
    );
}

#[tokio::test]
async fn create_node_errors() {
    let (_agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let rect = json!({ "name": "R", "kind": "rect" });
    c.err(
        "create_node",
        json!({ "parent_id": "n_zzzzzzzzzz", "node": rect }),
        "not_found",
    )
    .await;
    c.err(
        "create_node",
        json!({ "parent_id": PAGE, "node": { "id": "n_0000000000", "kind": "rect" } }),
        "invalid_args",
    )
    .await;
    c.err(
        "create_node",
        json!({ "parent_id": PAGE, "node": { "kind": "blob" } }),
        "invalid_args",
    )
    .await;
    // A rect cannot have children.
    let e = c
        .err(
            "create_node",
            json!({ "parent_id": BG, "node": rect }),
            "invalid_args",
        )
        .await;
    assert_eq!(e["node_id"], BG);
    c.err(
        "create_node",
        json!({ "parent_id": PAGE, "index": 99, "node": rect }),
        "invalid_args",
    )
    .await;
    let e = c
        .err(
            "create_node",
            json!({ "parent_id": LOCKED, "node": rect }),
            "locked",
        )
        .await;
    assert_eq!(e["node_id"], LOCKED);
    // A reference to a missing token breaks an invariant.
    c.err(
        "create_node",
        json!({ "parent_id": PAGE, "node": { "kind": "rect",
            "fills": [{ "type": "solid", "color": "{color.nope}" }] } }),
        "invariant",
    )
    .await;
}

#[tokio::test]
async fn update_node_patches_with_file_format_json() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let r = c
        .ok(
            "update_node",
            json!({ "id": BG, "patch": {
                "x": 30, "corner_radius": 8,
                "fills": [{ "type": "solid", "color": "#ff0000" }]
            } }),
        )
        .await;
    assert_eq!(r, json!({ "revision": 1, "id": BG }));
    let n = c.ok("get_node", json!({ "id": BG })).await;
    assert_eq!(n["node"]["x"], 30.0);
    assert_eq!(n["node"]["corner_radius"], 8.0);
    assert_eq!(n["node"]["fills"][0]["color"], "#ff0000");
    assert_eq!(agent.app().history()[0].0, "Agent: update Bg");

    // What get_node returns can be written back unchanged.
    let mut node = n["node"].as_object().unwrap().clone();
    for k in ["id", "kind"] {
        node.remove(k);
    }
    c.ok("update_node", json!({ "id": BG, "patch": node }))
        .await;

    let e = c
        .err(
            "update_node",
            json!({ "id": BG, "patch": { "colour": "#fff" } }),
            "invalid_patch",
        )
        .await;
    assert_eq!(e["node_id"], BG);
    let e = c
        .err(
            "update_node",
            json!({ "id": BG, "patch": { "content": "x" } }),
            "invalid_patch",
        )
        .await;
    assert!(e["message"].as_str().unwrap().contains("rect"), "{e}");
    c.err(
        "update_node",
        json!({ "id": "n_zzzzzzzzzz", "patch": {} }),
        "not_found",
    )
    .await;
    c.err(
        "update_node",
        json!({ "id": LOCKED_CHILD, "patch": { "x": 1 } }),
        "locked",
    )
    .await;
    c.err(
        "update_node",
        json!({ "id": BG, "patch": { "opacity": 3 } }),
        "invariant",
    )
    .await;
}

#[tokio::test]
async fn if_revision_guards_against_concurrent_edits() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    c.ok(
        "update_node",
        json!({ "id": BG, "patch": { "x": 1 }, "if_revision": 0 }),
    )
    .await;
    // The user edits meanwhile.
    agent.app().user_edit(vec![Command::SetProps {
        id: id(BG),
        patch: NodePatch {
            y: Some(5.0),
            ..NodePatch::default()
        },
    }]);
    let e = c
        .err(
            "update_node",
            json!({ "id": BG, "patch": { "x": 2 }, "if_revision": 1 }),
            "stale_revision",
        )
        .await;
    assert!(e["message"].as_str().unwrap().contains('2'), "{e}");
    assert_eq!(agent.app().doc().node(&id(BG)).unwrap().x, 1.0);
    c.ok(
        "update_node",
        json!({ "id": BG, "patch": { "x": 2 }, "if_revision": 2 }),
    )
    .await;
}

#[tokio::test]
async fn update_nodes_is_one_transaction() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let r = c
        .ok(
            "update_nodes",
            json!({ "updates": [
                { "id": BG, "patch": { "x": 1 } },
                { "id": TITLE, "patch": { "content": "Bye" } }
            ] }),
        )
        .await;
    assert_eq!(r, json!({ "revision": 1, "ids": [BG, TITLE] }));
    assert_eq!(agent.app().history().len(), 1);
    assert_eq!(agent.app().history()[0].0, "Agent: update 2 layers");
    // One bad patch fails the whole call and names the node.
    let e = c
        .err(
            "update_nodes",
            json!({ "updates": [
                { "id": BG, "patch": { "x": 5 } },
                { "id": BG, "patch": { "content": "x" } }
            ] }),
            "invalid_patch",
        )
        .await;
    assert_eq!(e["node_id"], BG);
    assert_eq!(agent.app().doc().node(&id(BG)).unwrap().x, 1.0);
}

#[tokio::test]
async fn delete_nodes() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let e = c
        .err("delete_nodes", json!({ "ids": [LOCKED_CHILD] }), "locked")
        .await;
    assert_eq!(e["node_id"], LOCKED_CHILD);
    // The last child of a group cannot go on its own.
    c.err("delete_nodes", json!({ "ids": [GROUP_CHILD] }), "invariant")
        .await;
    c.err(
        "delete_nodes",
        json!({ "ids": ["n_zzzzzzzzzz"] }),
        "not_found",
    )
    .await;
    // A node and one inside it: one delete covers both.
    let r = c
        .ok("delete_nodes", json!({ "ids": [GROUP, GROUP_CHILD, BG] }))
        .await;
    assert_eq!(r["deleted"], json!([GROUP, GROUP_CHILD, BG]));
    let doc = agent.app().doc();
    assert!(doc.node(&id(GROUP)).is_none() && doc.node(&id(BG)).is_none());
    assert_eq!(agent.app().history()[0].0, "Agent: delete 3 layers");
}

#[tokio::test]
async fn move_node_reparents_reorders_and_repositions() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    c.ok("move_node", json!({ "id": BG, "index": 2 })).await;
    let doc = agent.app().doc();
    let kids = doc.node(&id(SCREEN)).unwrap().children().unwrap();
    assert_eq!(kids[2].id, id(BG));
    c.ok(
        "move_node",
        json!({ "id": BG, "parent_id": PAGE2, "x": 7, "y": 8 }),
    )
    .await;
    let doc = agent.app().doc();
    assert_eq!(doc.pages[1].children[0].id, id(BG));
    assert_eq!(
        (doc.pages[1].children[0].x, doc.pages[1].children[0].y),
        (7.0, 8.0)
    );
    assert_eq!(agent.app().history().len(), 2);
    assert_eq!(agent.app().history()[1].0, "Agent: move Bg");

    c.err("move_node", json!({ "id": BG }), "invalid_args")
        .await;
    c.err(
        "move_node",
        json!({ "id": SCREEN, "parent_id": TITLE }),
        "invalid_args",
    )
    .await;
    c.err(
        "move_node",
        json!({ "id": SCREEN, "parent_id": BG.replace("bg", "zz") }),
        "not_found",
    )
    .await;
    c.err(
        "move_node",
        json!({ "id": TITLE, "parent_id": LOCKED }),
        "locked",
    )
    .await;
}

#[tokio::test]
async fn duplicate_node_copies_with_fresh_ids() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let r = c
        .ok("duplicate_node", json!({ "id": GROUP, "offset": [20, 0] }))
        .await;
    let copy_id = id(r["id"].as_str().unwrap());
    assert_ne!(copy_id, id(GROUP));
    let doc = agent.app().doc();
    let kids = doc.node(&id(SCREEN)).unwrap().children().unwrap();
    assert_eq!(kids[3].id, copy_id, "just above the original");
    assert_eq!(kids[3].x, 20.0);
    assert_ne!(kids[3].children().unwrap()[0].id, id(GROUP_CHILD));
    assert_eq!(agent.app().history()[0].0, "Agent: duplicate Group");
    let r = c
        .ok(
            "duplicate_node",
            json!({ "id": LOCKED, "parent_id": PAGE2 }),
        )
        .await;
    assert_eq!(
        agent.app().doc().pages[1].children[0].id,
        id(r["id"].as_str().unwrap())
    );
    c.err(
        "duplicate_node",
        json!({ "id": BG, "parent_id": LOCKED }),
        "locked",
    )
    .await;
}

#[tokio::test]
async fn tokens_and_styles() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    c.ok(
        "set_tokens",
        json!({ "colors": { "color.accent": "#ff00ff" }, "spacing": { "space.4": 16 } }),
    )
    .await;
    let t = c.ok("get_tokens", json!({})).await;
    assert_eq!(t["tokens"]["colors"]["color.accent"], "#ff00ff");
    assert_eq!(t["tokens"]["spacing"]["space.4"], 16.0);
    c.ok("set_tokens", json!({ "colors": { "color.accent": null } }))
        .await;
    let t = c.ok("get_tokens", json!({})).await;
    assert!(t["tokens"]["colors"].get("color.accent").is_none());
    c.err(
        "set_tokens",
        json!({ "colors": { "c": "red" } }),
        "invalid_args",
    )
    .await;
    c.err("set_tokens", json!({}), "invalid_args").await;

    // Create a style, change it, apply it, detach it.
    let r = c
        .ok(
            "upsert_style",
            json!({ "name": "Brand/Accent", "kind": "fill",
                    "value": { "fills": [{ "type": "solid", "color": "#00ff00" }] } }),
        )
        .await;
    let style = r["id"].as_str().unwrap().to_string();
    assert!(style.starts_with("s_"));
    c.ok(
        "upsert_style",
        json!({ "id": style, "name": "Brand/Accent", "kind": "fill",
                "value": { "fills": [{ "type": "solid", "color": "#0000ff" }] } }),
    )
    .await;
    c.err(
        "upsert_style",
        json!({ "id": "s_zzzzzzzzzz", "name": "X", "kind": "fill", "value": {} }),
        "not_found",
    )
    .await;
    c.err(
        "upsert_style",
        json!({ "name": "X", "kind": "fill", "value": { "fills": "red" } }),
        "invalid_args",
    )
    .await;

    let r = c
        .ok(
            "apply_style",
            json!({ "node_ids": [BG, SCREEN], "style_id": style }),
        )
        .await;
    assert_eq!(r["ids"], json!([BG, SCREEN]));
    let n = c.ok("get_node", json!({ "id": BG })).await;
    assert_eq!(n["node"]["fill_style"], style);
    // Detach: the style's fills are copied onto the node.
    c.ok(
        "apply_style",
        json!({ "node_ids": [BG], "style_id": null, "slot": "fill" }),
    )
    .await;
    let doc = agent.app().doc();
    let shape = doc.node(&id(BG)).unwrap().shape().unwrap().clone();
    assert_eq!(shape.fill_style, None);
    assert!(
        matches!(&shape.fills[0], Paint::Solid(p) if p.color == doc::ColorRef::literal("#0000ff"))
    );
    let labels: Vec<String> = agent.app().history().into_iter().map(|h| h.0).collect();
    assert_eq!(
        labels,
        [
            "Agent: set tokens",
            "Agent: set tokens",
            "Agent: create style Brand/Accent",
            "Agent: update style Brand/Accent",
            "Agent: apply style Brand/Accent",
            "Agent: detach style from Bg",
        ]
    );

    c.err(
        "apply_style",
        json!({ "node_ids": [BG], "style_id": "s_zzzzzzzzzz" }),
        "not_found",
    )
    .await;
    c.err(
        "apply_style",
        json!({ "node_ids": [TITLE], "style_id": FILL_STYLE, "slot": "stroke" }),
        "invalid_patch",
    )
    .await;
    c.err(
        "apply_style",
        json!({ "node_ids": [LOCKED], "style_id": FILL_STYLE }),
        "locked",
    )
    .await;
    // Removing a token that is still used is refused.
    c.ok(
        "update_node",
        json!({ "id": BG, "patch": {
        "fills": [{ "type": "solid", "color": "{color.primary}" }] } }),
    )
    .await;
    c.err(
        "set_tokens",
        json!({ "colors": { "color.primary": null } }),
        "invariant",
    )
    .await;
}

#[tokio::test]
async fn set_selection_points_at_nodes() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let r = c
        .ok("set_selection", json!({ "ids": [TITLE], "zoom_to": true }))
        .await;
    assert_eq!(r, json!({ "ids": [TITLE] }));
    assert_eq!(agent.app().selection(), [id(TITLE)]);
    assert!(agent.app().history().is_empty(), "not in history");
    c.err(
        "set_selection",
        json!({ "ids": ["n_zzzzzzzzzz"] }),
        "not_found",
    )
    .await;
}

// -- Look and export --------------------------------------------------------

#[tokio::test]
async fn screenshot_returns_png_image_content() {
    let (_agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let r = c
        .call(
            "screenshot",
            json!({ "id": SCREEN, "scale": 2, "max_size": 400 }),
        )
        .await;
    assert!(!r.is_error, "{}", r.json);
    let image = &r.raw["content"][0];
    assert_eq!(image["type"], "image");
    assert_eq!(image["mimeType"], "image/png");
    let png = base64::engine::general_purpose::STANDARD
        .decode(image["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    // 400x300 at scale 2 is clamped to 400 on the longest side.
    assert_eq!(r.json, json!({ "width": 400, "height": 300, "scale": 1.0 }));

    c.err("screenshot", json!({ "id": "n_zzzzzzzzzz" }), "not_found")
        .await;
    c.err(
        "screenshot",
        json!({ "id": SCREEN, "scale": 0 }),
        "invalid_args",
    )
    .await;
    c.err(
        "screenshot",
        json!({ "id": SCREEN, "background": "white" }),
        "invalid_args",
    )
    .await;
    // Locked nodes can still be looked at.
    c.ok("screenshot", json!({ "id": LOCKED })).await;
}

#[tokio::test]
async fn export_file_writes_inside_the_document_directory_only() {
    let dir = temp_dir("export");
    let app = FakeApp::new(fixture(), Some(dir.join("doc.god.json")));
    let (_agent, mut c) = start(app, "t").await;
    // `..` through a directory that does not exist cannot be resolved.
    c.err(
        "export_file",
        json!({ "id": SCREEN, "format": "svg", "path": "out/../screen.svg" }),
        "invalid_args",
    )
    .await;
    let r = c
        .ok(
            "export_file",
            json!({ "id": SCREEN, "format": "png", "path": "screen.png" }),
        )
        .await;
    let written = dir.canonicalize().unwrap().join("screen.png");
    assert_eq!(
        r["bytes"],
        std::fs::metadata(dir.join("screen.png")).unwrap().len()
    );
    assert!(r["path"].as_str().unwrap().ends_with("screen.png"));
    assert!(written.exists());
    let abs = dir.join("screen.svg");
    c.ok(
        "export_file",
        json!({ "id": SCREEN, "format": "svg", "path": abs }),
    )
    .await;
    assert!(std::fs::read_to_string(&abs).unwrap().starts_with("<svg"));

    c.err(
        "export_file",
        json!({ "id": SCREEN, "format": "png", "path": "/proc/x.png" }),
        "invalid_args",
    )
    .await;
    c.err(
        "export_file",
        json!({ "id": SCREEN, "format": "png", "path": "../escape.png" }),
        "invalid_args",
    )
    .await;
    c.err(
        "export_file",
        json!({ "id": SCREEN, "format": "jpg", "path": "a.jpg" }),
        "invalid_args",
    )
    .await;
    for path in [
        "screen.svg",
        ".hidden.png",
        ".git/x.png",
        "out/.cache/x.png",
    ] {
        c.err(
            "export_file",
            json!({ "id": SCREEN, "format": "png", "path": path }),
            "invalid_args",
        )
        .await;
    }
    assert!(!dir.join(".hidden.png").exists());
    c.err(
        "export_file",
        json!({ "id": "n_zzzzzzzzzz", "format": "png", "path": "a.png" }),
        "not_found",
    )
    .await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn export_file_needs_an_absolute_path_for_unsaved_documents() {
    let (_agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    c.err(
        "export_file",
        json!({ "id": SCREEN, "format": "png", "path": "a.png" }),
        "invalid_args",
    )
    .await;
}

// -- Modes ------------------------------------------------------------------

#[tokio::test]
async fn read_only_mode_refuses_edits_but_not_reads() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    agent.set_read_only(true);
    c.err(
        "update_node",
        json!({ "id": BG, "patch": { "x": 1 } }),
        "read_only",
    )
    .await;
    c.err(
        "create_node",
        json!({ "parent_id": PAGE, "node": { "kind": "rect" } }),
        "read_only",
    )
    .await;
    c.ok("get_node", json!({ "id": BG })).await;
    c.ok("set_selection", json!({ "ids": [BG] })).await;
    c.ok("screenshot", json!({ "id": BG })).await;
    agent.set_read_only(false);
    c.ok("update_node", json!({ "id": BG, "patch": { "x": 1 } }))
        .await;
}

// -- Prompts and resources --------------------------------------------------

#[tokio::test]
async fn design_conventions_prompt() {
    let (_agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let list = c.result("prompts/list", json!({})).await;
    assert_eq!(list["prompts"][0]["name"], "design_conventions");
    let p = c
        .result("prompts/get", json!({ "name": "design_conventions" }))
        .await;
    let text = p["messages"][0]["content"]["text"].as_str().unwrap();
    for needle in [
        "auto-layout",
        "hug",
        "{color.primary}",
        "screenshot",
        "undo",
        "get_selection",
    ] {
        assert!(text.contains(needle), "prompt mentions {needle}");
    }
    let resp = c.request("prompts/get", json!({ "name": "nope" })).await;
    assert!(resp.get("error").is_some(), "{resp}");
}

#[tokio::test]
async fn resources_read_document_and_selection() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let list = c.result("resources/list", json!({})).await;
    let uris: Vec<&str> = list["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris, ["design://document", "design://selection"]);

    agent.app().user_select(&[TITLE]);
    let r = c
        .result("resources/read", json!({ "uri": "design://selection" }))
        .await;
    let body: Value = serde_json::from_str(r["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["ids"], json!([TITLE]));
    assert_eq!(r["contents"][0]["mimeType"], "application/json");
    let r = c
        .result("resources/read", json!({ "uri": "design://document" }))
        .await;
    let body: Value = serde_json::from_str(r["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["title"], "Fixture");
    let resp = c
        .request("resources/read", json!({ "uri": "design://nope" }))
        .await;
    assert!(resp.get("error").is_some(), "{resp}");
}

fn updated_uri(n: &Value) -> &str {
    assert_eq!(n["method"], "notifications/resources/updated", "{n}");
    n["params"]["uri"].as_str().unwrap()
}

#[tokio::test]
async fn subscriptions_are_notified_once_per_burst() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    c.result("resources/subscribe", json!({ "uri": "design://document" }))
        .await;
    // A burst of commits (from another agent session) gives one notification.
    let mut other = connect(&agent);
    other.initialize("other").await;
    for x in 1..=3 {
        other
            .ok("update_node", json!({ "id": BG, "patch": { "x": x } }))
            .await;
    }
    let n = c
        .next_notification(Duration::from_secs(2))
        .await
        .expect("document notification");
    assert_eq!(updated_uri(&n), "design://document");
    assert!(
        c.next_notification(Duration::from_millis(600))
            .await
            .is_none(),
        "debounced to one"
    );
    // Not subscribed to the selection: no notification for it.
    agent.app().user_select(&[BG]);
    assert!(c
        .next_notification(Duration::from_millis(600))
        .await
        .is_none());
    // Subscribed: one, after the debounce.
    c.result(
        "resources/subscribe",
        json!({ "uri": "design://selection" }),
    )
    .await;
    let start = tokio::time::Instant::now();
    agent.app().user_select(&[TITLE]);
    let n = c
        .next_notification(Duration::from_secs(2))
        .await
        .expect("selection notification");
    assert_eq!(updated_uri(&n), "design://selection");
    assert!(start.elapsed() >= Duration::from_millis(200), "debounced");
    // Unsubscribe stops them.
    c.result(
        "resources/unsubscribe",
        json!({ "uri": "design://selection" }),
    )
    .await;
    agent.app().user_select(&[BG]);
    assert!(c
        .next_notification(Duration::from_millis(600))
        .await
        .is_none());
    let resp = c
        .request("resources/subscribe", json!({ "uri": "design://nope" }))
        .await;
    assert!(resp.get("error").is_some(), "{resp}");
}

#[tokio::test]
async fn sessions_are_labelled_and_removed_on_disconnect() {
    let agent = agent::Agent::new(FakeApp::new(fixture(), None));
    let mut a = connect(&agent);
    a.initialize("alpha").await;
    let mut b = connect(&agent);
    b.initialize("beta").await;
    a.ok("update_node", json!({ "id": BG, "patch": { "x": 1 } }))
        .await;
    b.ok("update_node", json!({ "id": BG, "patch": { "x": 2 } }))
        .await;
    let origins: Vec<Origin> = agent.app().history().into_iter().map(|h| h.1).collect();
    assert_eq!(origins, [agent_origin("alpha"), agent_origin("beta")]);
    let clients: Vec<String> = agent.sessions().into_iter().map(|s| s.client).collect();
    assert_eq!(clients, ["alpha", "beta"]);

    drop(a);
    let gone = async {
        while agent.sessions().len() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(2), gone)
        .await
        .expect("closed session removed");
    let beta = agent.sessions()[0].id;
    agent.disconnect(beta);
    let gone = async {
        while !agent.sessions().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(2), gone)
        .await
        .expect("disconnected session removed");
}

#[tokio::test]
async fn create_component_and_instance_are_single_undo_steps() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let r = c
        .ok(
            "create_component",
            json!({ "from_id": SCREEN, "name": "Cards/Screen" }),
        )
        .await;
    assert_eq!(r["instance_id"], SCREEN);
    let comp = r["component_id"].as_str().unwrap().to_string();
    let doc = agent.app().doc();
    assert_eq!(doc.components.len(), 1);
    assert_eq!(doc.components[0].id.as_str(), comp);
    assert_eq!(doc.components[0].name, "Cards/Screen");
    assert!(matches!(
        doc.node(&id(SCREEN)).unwrap().kind,
        NodeKind::Instance(_)
    ));
    let h = agent.app().history();
    assert_eq!(h.len(), 1, "name and creation are one transaction");
    assert_eq!(h[0].0, "Agent: create component Screen");
    assert_eq!(h[0].1, agent_origin("t"));

    let title_in_master = doc.components[0].root.children().unwrap()[1].id.to_string();
    let r = c
        .ok(
            "create_instance",
            json!({
                "component_id": comp, "parent_id": PAGE2, "x": 30, "y": 40,
                "overrides": { title_in_master.clone(): { "type": "text", "content": "Bye" } },
                "select": true,
            }),
        )
        .await;
    let new = id(r["id"].as_str().unwrap());
    let doc = agent.app().doc();
    let n = doc.node(&new).unwrap();
    assert_eq!((n.x, n.y, n.width, n.height), (30.0, 40.0, 400.0, 300.0));
    match &n.kind {
        NodeKind::Instance(i) => {
            assert_eq!(i.component.as_str(), comp);
            assert_eq!(i.overrides.len(), 1);
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(agent.app().selection(), vec![new.clone()]);
    assert_eq!(agent.app().history().len(), 2);

    agent.app().undo();
    assert!(agent.app().doc().node(&new).is_none());
    agent.app().undo();
    let doc = agent.app().doc();
    assert!(doc.components.is_empty());
    assert!(matches!(
        doc.node(&id(SCREEN)).unwrap().kind,
        NodeKind::Frame(_)
    ));
}

#[tokio::test]
async fn component_tool_errors() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    c.err(
        "create_component",
        json!({ "from_id": "n_nope000001" }),
        "not_found",
    )
    .await;
    c.err("create_component", json!({ "from_id": TITLE }), "invariant")
        .await;
    c.err("create_component", json!({ "from_id": LOCKED }), "locked")
        .await;
    c.err(
        "create_instance",
        json!({ "component_id": "c_nope000001", "parent_id": PAGE }),
        "not_found",
    )
    .await;
    let r = c.ok("create_component", json!({ "from_id": SCREEN })).await;
    let comp = r["component_id"].as_str().unwrap();
    c.err(
        "create_instance",
        json!({ "component_id": comp, "parent_id": "n_nope000001" }),
        "not_found",
    )
    .await;
    c.err(
        "create_instance",
        json!({ "component_id": comp, "parent_id": LOCKED }),
        "locked",
    )
    .await;
    c.err(
        "create_instance",
        json!({ "component_id": comp, "parent_id": PAGE,
                "overrides": { "n_xx00000001": { "type": "bogus" } } }),
        "invalid_args",
    )
    .await;
    assert_eq!(agent.app().history().len(), 1, "failures change nothing");
}

#[tokio::test]
async fn export_code_returns_files_for_each_format() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let revision = agent.app().revision();
    let r = c
        .ok("export_code", json!({ "id": SCREEN, "format": "html" }))
        .await;
    let files = r["files"].as_array().unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["index.html", "styles.css"]);
    assert_eq!(files[0]["language"], "html");
    assert!(files[0]["content"]
        .as_str()
        .unwrap()
        .contains("Hello world"));
    let tw = c
        .ok(
            "export_code",
            json!({ "id": SCREEN, "format": "tailwind", "options": { "tokens": "inline" } }),
        )
        .await;
    assert!(tw["files"][0]["content"]
        .as_str()
        .unwrap()
        .contains("class="));
    let react = c
        .ok(
            "export_code",
            json!({ "id": SCREEN, "format": "react",
                    "options": { "responsive": true, "include_components": false } }),
        )
        .await;
    assert!(react["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["language"] == "tsx"));
    // Reading is not an edit.
    assert_eq!(agent.app().revision(), revision);
    assert!(agent.app().history().is_empty());
}

#[tokio::test]
async fn export_code_draws_instances_with_their_overrides() {
    let (agent, mut c) = start(FakeApp::new(fixture(), None), "t").await;
    let r = c.ok("create_component", json!({ "from_id": SCREEN })).await;
    let comp = r["component_id"].as_str().unwrap().to_string();
    let master = agent.app().doc().components[0].root.children().unwrap()[1]
        .id
        .to_string();
    let inst = c
        .ok(
            "create_instance",
            json!({
                "component_id": comp, "parent_id": PAGE,
                "overrides": { master: { "type": "text", "content": "Overridden title" } },
            }),
        )
        .await;
    let html = c
        .ok("export_code", json!({ "id": inst["id"], "format": "html" }))
        .await;
    let text = html["files"][0]["content"].as_str().unwrap();
    assert!(text.contains("Overridden title"), "{text}");
    assert!(!text.contains("Hello world"), "{text}");
    c.err(
        "export_code",
        json!({ "id": "n_nope000001", "format": "html" }),
        "not_found",
    )
    .await;
    c.err(
        "export_code",
        json!({ "id": SCREEN, "format": "vue" }),
        "invalid_args",
    )
    .await;
}
