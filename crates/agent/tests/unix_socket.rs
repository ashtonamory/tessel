//! The same protocol over real unix sockets: a `UnixStream` pair, and the
//! listener the app binds (§6.1).

mod common;

use agent::Agent;
use common::*;
use serde_json::json;
use tokio::net::{UnixListener, UnixStream};

#[tokio::test]
async fn session_over_a_unix_stream_pair() {
    let app = FakeApp::new(fixture(), None);
    let before = app.doc();
    let agent = Agent::new(app);
    let (server, client) = UnixStream::pair().unwrap();
    let a = agent.clone();
    tokio::spawn(async move { a.serve(server).await });
    let mut c = Client::new(client);
    c.initialize("claude-code").await;
    let tools = c.result("tools/list", json!({})).await;
    assert_eq!(tools["tools"].as_array().unwrap().len(), 20);

    let r = c
        .ok(
            "create_node",
            json!({ "parent_id": PAGE, "node": { "name": "Screen 2", "kind": "frame",
                "width": 360, "height": 640,
                "children": [{ "name": "Header", "kind": "frame", "width": 360, "height": 56 }] } }),
        )
        .await;
    let shot = c.call("screenshot", json!({ "id": r["id"] })).await;
    assert!(!shot.is_error);
    assert_eq!(
        shot.json,
        json!({ "width": 360, "height": 640, "scale": 1.0 })
    );
    c.err(
        "update_node",
        json!({ "id": LOCKED, "patch": { "x": 0 } }),
        "locked",
    )
    .await;
    // The whole build is one Ctrl+Z.
    agent.app().undo();
    assert_eq!(agent.app().doc(), before);
}

#[tokio::test]
async fn listener_serves_concurrent_sessions() {
    let dir = temp_dir("socket");
    let path = dir.join("test.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let agent = Agent::new(FakeApp::new(fixture(), None));
    let a = agent.clone();
    tokio::spawn(async move { a.serve_unix(listener).await });

    let mut one = Client::new(UnixStream::connect(&path).await.unwrap());
    let mut two = Client::new(UnixStream::connect(&path).await.unwrap());
    one.initialize("one").await;
    two.initialize("two").await;
    let (a, b) = tokio::join!(
        one.call("update_node", json!({ "id": BG, "patch": { "x": 1 } })),
        two.call("update_node", json!({ "id": TITLE, "patch": { "x": 2 } })),
    );
    assert!(!a.is_error && !b.is_error);
    let mut revisions = [a.json["revision"].as_u64(), b.json["revision"].as_u64()];
    revisions.sort();
    assert_eq!(revisions, [Some(1), Some(2)], "edits are serialized");
    let mut clients: Vec<String> = agent.sessions().into_iter().map(|s| s.client).collect();
    clients.sort();
    assert_eq!(clients, ["one", "two"]);
    let _ = std::fs::remove_dir_all(dir);
}
