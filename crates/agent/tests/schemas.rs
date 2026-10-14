//! Tool schemas are generated from the argument types (§6.2). This snapshot
//! makes every schema change deliberate: if it fails, check the diff and
//! rerun with `UPDATE_SNAPSHOTS=1` to accept it.

use std::path::Path;

#[test]
fn tool_schemas_match_snapshot() {
    let tools: Vec<_> = agent::tools::list()
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": *t.input_schema,
                "annotations": t.annotations,
            })
        })
        .collect();
    let actual = serde_json::to_string_pretty(&tools).unwrap() + "\n";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/tools.json");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        expected == actual,
        "tool schemas changed; if on purpose, run with UPDATE_SNAPSHOTS=1.\n\
         --- actual ---\n{actual}"
    );
}
