//! Reading and writing `.tessel.json` files (§4).

use std::fs;
use std::io::Write;
use std::path::Path;

use serde_json::Value;

use crate::canonical::to_canonical_string;
use crate::model::{Document, FORMAT_NAME, FORMAT_VERSION, LEGACY_FORMAT_NAME};
use crate::validate::{validate, Problem};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("not valid JSON: {0}")]
    Json(#[source] serde_json::Error),
    #[error("not a {FORMAT_NAME} document: {0}")]
    NotADocument(String),
    #[error("the file is format version {found}; this app reads up to version {supported}. Update Tessel to open it.")]
    TooNew { found: u64, supported: u32 },
    #[error("document does not match the format: {0}")]
    Shape(#[source] serde_json::Error),
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),
}

/// A loaded document plus the invariant violations found in it. A document
/// with problems is still usable (the app opens it read-only, §3.9).
#[derive(Debug)]
pub struct Loaded {
    pub doc: Document,
    pub problems: Vec<Problem>,
}

/// Migration steps; `MIGRATIONS[v - 1]` turns version `v` into `v + 1`.
/// Version 1 is the first, so there are none yet.
const MIGRATIONS: &[fn(Value) -> Value] = &[];

/// Bring a parsed file up to the current format version (§4.4).
pub fn migrate(mut v: Value) -> Result<Value, LoadError> {
    let format = v
        .get_mut("format")
        .ok_or_else(|| LoadError::NotADocument("missing \"format\"".into()))?;
    match format.get("name").and_then(Value::as_str) {
        Some(FORMAT_NAME) => {}
        Some(LEGACY_FORMAT_NAME) => format["name"] = Value::from(FORMAT_NAME),
        _ => {
            return Err(LoadError::NotADocument(format!(
                "format.name must be {FORMAT_NAME:?}"
            )))
        }
    }
    let version = format
        .get("version")
        .and_then(Value::as_u64)
        .filter(|v| *v >= 1)
        .ok_or_else(|| LoadError::NotADocument("format.version must be an integer >= 1".into()))?;
    if version > FORMAT_VERSION as u64 {
        return Err(LoadError::TooNew {
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    for step in &MIGRATIONS[version as usize - 1..] {
        v = step(v);
    }
    if let Some(f) = v.get_mut("format") {
        f["version"] = Value::from(FORMAT_VERSION);
    }
    Ok(v)
}

/// Parse, migrate, deserialize and validate a document.
pub fn load_str(s: &str) -> Result<Loaded, LoadError> {
    let v: Value = serde_json::from_str(s).map_err(LoadError::Json)?;
    let v = migrate(v)?;
    let mut doc: Document = serde_json::from_value(v).map_err(LoadError::Shape)?;
    doc.rebuild_index();
    let problems = validate(&doc);
    Ok(Loaded { doc, problems })
}

pub fn load_file(path: &Path) -> Result<Loaded, LoadError> {
    load_str(&fs::read_to_string(path)?)
}

/// The canonical text of a document (§4.3).
pub fn save_string(doc: &Document) -> String {
    to_canonical_string(doc).expect("documents always serialize")
}

/// Write atomically: `<file>.tmp`, fsync, rename (§4.5).
pub fn save_file(path: &Path, doc: &Document) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = Path::new(&tmp);
    {
        let mut f = fs::File::create(tmp)?;
        f.write_all(save_string(doc).as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_documents_and_newer_versions() {
        assert!(matches!(load_str("{"), Err(LoadError::Json(_))));
        assert!(matches!(load_str("{}"), Err(LoadError::NotADocument(_))));
        assert!(matches!(
            load_str(r#"{ "format": { "name": "other", "version": 1 } }"#),
            Err(LoadError::NotADocument(_))
        ));
        assert!(matches!(
            load_str(r#"{ "format": { "name": "tessel", "version": 2 } }"#),
            Err(LoadError::TooNew { found: 2, .. })
        ));
        assert!(matches!(
            load_str(r#"{ "format": { "name": "tessel", "version": 1 }, "pages": 3 }"#),
            Err(LoadError::Shape(_))
        ));
    }

    #[test]
    fn legacy_format_name_loads_and_saves_as_tessel() {
        let l = load_str(r#"{ "format": { "name": "game-on-design", "version": 1 } }"#).unwrap();
        assert!(l.problems.is_empty(), "{:?}", l.problems);
        assert_eq!(l.doc.format.name, FORMAT_NAME);
        assert!(save_string(&l.doc).contains(r#""name": "tessel""#));
    }

    #[test]
    fn minimal_document_loads() {
        let l = load_str(r#"{ "format": { "name": "tessel", "version": 1 } }"#).unwrap();
        assert!(l.problems.is_empty(), "{:?}", l.problems);
        assert_eq!(
            save_string(&l.doc),
            "{\n  \"format\": { \"name\": \"tessel\", \"version\": 1 },\n  \"pages\": []\n}\n"
        );
    }

    #[test]
    fn save_file_is_atomic_and_round_trips() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(format!("doc-io-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.tessel.json");
        let doc = load_str(r#"{ "format": { "name": "tessel", "version": 1 } }"#)
            .unwrap()
            .doc;
        save_file(&path, &doc).unwrap();
        assert!(!dir.join("t.tessel.json.tmp").exists());
        assert_eq!(load_file(&path).unwrap().doc, doc);
        fs::remove_dir_all(&dir).unwrap();
    }
}
