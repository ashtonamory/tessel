//! `AppHandle`: the agent's only view of the app (§2, §5.4). The app
//! implements it on the UI thread's side of a channel; tests implement it
//! over a plain `Document` + `History`.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use doc::{Color, Command, Document, NodeId, PageId, ParentRef};
use layout::Geometry;

use crate::error::ToolError;

/// What the user has selected, and on which page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionState {
    /// The page the window shows.
    pub page_id: Option<PageId>,
    pub ids: Vec<NodeId>,
}

/// The state reads are served from: published by the app after each
/// committed transaction or selection change, so reads never wait for a
/// frame (§5.4).
#[derive(Clone)]
pub struct Snapshot {
    pub document: Document,
    pub geometry: Geometry,
    pub selection: SelectionState,
    /// Bumped by every committed transaction (§6.2).
    pub revision: u64,
    /// Unsaved changes.
    pub dirty: bool,
    /// Where the document is saved, if it has been.
    pub path: Option<PathBuf>,
    /// Image bytes, for exports that write image files.
    pub assets: doc::Assets,
}

/// One tool call's edits: applied as one transaction, one undo step (§5.3).
#[derive(Clone, Debug, PartialEq)]
pub struct TxRequest {
    pub commands: Vec<Command>,
    /// "Agent: create pricing section".
    pub label: String,
    /// The client name from `initialize`, for `Origin::Agent { client }`.
    pub client: String,
    /// Fail with `stale_revision` unless the document is at this revision.
    pub if_revision: Option<u64>,
    /// Select these nodes when the transaction commits.
    pub select: Option<Vec<NodeId>>,
}

/// A committed transaction.
#[derive(Clone, Debug, PartialEq)]
pub struct TxOk {
    pub revision: u64,
    /// The node IDs the transaction changed (§5.5).
    pub touched: BTreeSet<NodeId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenshotRequest {
    pub node: NodeId,
    pub scale: f32,
    /// Longest side in pixels; the scale is reduced to fit.
    pub max_size: u32,
    /// Painted behind the node; transparent if `None`.
    pub background: Option<Color>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The scale actually used, after fitting `max_size`.
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Png,
    Svg,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportRequest {
    pub node: NodeId,
    pub format: ExportFormat,
    /// Already passed [`check_export_path`]; the app checks it again.
    pub path: PathBuf,
    pub scale: f32,
}

/// The app, as the agent sees it. Every method may be called from any tokio
/// task; edits are serialized by the app (§5.4).
pub trait AppHandle: Send + Sync + 'static {
    /// The latest published state. Must not wait for the UI thread.
    fn snapshot(&self) -> Arc<Snapshot>;

    /// Apply `tx.commands` as one transaction. Errors: `stale_revision`
    /// when `if_revision` does not match, `locked` (see [`check_locked`]),
    /// or the command's own error.
    fn submit(&self, tx: TxRequest) -> impl Future<Output = Result<TxOk, ToolError>> + Send;

    /// Replace the selection (not a document change, not in history).
    /// Returns the IDs selected.
    fn set_selection(
        &self,
        ids: Vec<NodeId>,
        zoom_to: bool,
    ) -> impl Future<Output = Result<Vec<NodeId>, ToolError>> + Send;

    /// Render a node exactly as the canvas shows it, without selection
    /// chrome, to PNG.
    fn screenshot(
        &self,
        req: ScreenshotRequest,
    ) -> impl Future<Output = Result<Screenshot, ToolError>> + Send;

    /// Export a node to a file. Returns the number of bytes written.
    fn export_file(
        &self,
        req: ExportRequest,
    ) -> impl Future<Output = Result<u64, ToolError>> + Send;

    /// Whether this handle works on files by path and offers the document
    /// tools (`list_documents`, `open_document`, `new_document`,
    /// `import_image`): headless mode does (§6.7); the app, with its one open
    /// document, does not.
    fn has_documents(&self) -> bool {
        false
    }

    /// Run a document tool. Only called when [`AppHandle::has_documents`].
    fn documents(
        &self,
        call: DocumentCall,
    ) -> impl Future<Output = Result<serde_json::Value, ToolError>> + Send {
        let _ = call;
        async {
            Err(ToolError::invalid_args(
                "document tools need `tessel mcp --headless`",
            ))
        }
    }
}

/// A document tool call (headless mode, §6.7). Paths are as the agent gave
/// them; the handle resolves and checks them.
#[derive(Clone, Debug, PartialEq)]
pub enum DocumentCall {
    List,
    Open {
        path: String,
    },
    New {
        path: String,
        template: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
        title: Option<String>,
        background: Option<String>,
    },
    ImportImage {
        path: String,
        scale: Option<u32>,
    },
}

/// The locked node that is `id` or contains it, if any.
pub fn locked_at(doc: &Document, id: &NodeId) -> Option<NodeId> {
    let mut cur = Some(id.clone());
    while let Some(id) = cur {
        let node = doc.node(&id)?;
        if node.locked {
            return Some(id);
        }
        cur = match doc.parent_of(&id)? {
            (ParentRef::Node(p), _) => Some(p),
            (ParentRef::Page(_), _) => None,
        };
    }
    None
}

/// Fail with `locked` if any command edits, deletes, moves or adds children
/// to a node the user has locked, or one inside it (§5.4). The agent checks
/// this against its snapshot; the app should check again when it applies.
pub fn check_locked(doc: &Document, commands: &[Command]) -> Result<(), ToolError> {
    let check = |id: &NodeId| match locked_at(doc, id) {
        Some(_) => Err(ToolError::locked(id)),
        None => Ok(()),
    };
    let parent = |p: &ParentRef| match p {
        ParentRef::Node(id) => check(id),
        ParentRef::Page(_) => Ok(()),
    };
    for c in commands {
        match c {
            Command::CreateNode { parent: p, .. } => parent(p)?,
            Command::DeleteNode { id }
            | Command::SetProps { id, .. }
            | Command::SetGeometry { id, .. }
            | Command::CreateComponent { from: id }
            | Command::DetachInstance { id } => check(id)?,
            Command::MoveNode { id, parent: p, .. } => {
                check(id)?;
                parent(p)?;
            }
            Command::Batch(cmds) => check_locked(doc, cmds)?,
            _ => {}
        }
    }
    Ok(())
}

impl ExportFormat {
    /// The file extension the format must be written with.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Svg => "svg",
        }
    }
}

/// Check that `export_file` may write `format` to `path`, and return the
/// path to write: absolute, with its existing ancestors resolved (§6.3).
/// The agent checks this before asking the app, and the app checks again
/// right before writing. The rules:
///
/// - the extension matches the format (`.png` or `.svg`, any case);
/// - the file is inside the document's directory or `home`, after resolving
///   `..` and symlinks in the ancestors that exist; a `..` in the part that
///   does not exist yet is refused;
/// - no folder or file below that root starts with `.`, so hidden folders
///   and dotfiles are out of reach;
/// - the file itself is never a symlink, and an existing file is only
///   replaced if it is a regular file (its extension already matches).
pub fn check_export_path(
    path: &Path,
    format: ExportFormat,
    doc_path: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, ToolError> {
    let shown = path.display();
    if !path.is_absolute() {
        return Err(ToolError::invalid_args(format!(
            "{shown} is not an absolute path"
        )));
    }
    let ext = format.extension();
    let name = match path.file_name() {
        Some(n)
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(ext)) =>
        {
            n
        }
        _ => {
            return Err(ToolError::invalid_args(format!(
                "{shown}: a {} export must be written to a .{ext} file",
                ext.to_uppercase()
            )))
        }
    };
    // The parent only: the file itself must not be followed if it is a
    // symlink.
    let Some(parent) = path.parent().and_then(resolve_existing_prefix) else {
        return Err(ToolError::invalid_args(format!(
            "{shown}: `..` is only allowed through folders that exist"
        )));
    };
    let resolved = parent.join(name);
    let roots: Vec<PathBuf> = doc_path
        .and_then(Path::parent)
        .into_iter()
        .chain(home)
        .filter_map(|root| root.canonicalize().ok())
        .filter(|root| resolved.starts_with(root))
        .collect();
    if roots.is_empty() {
        return Err(ToolError::invalid_args(format!(
            "{shown} is outside the document's folder and your home folder"
        )));
    }
    let visible_below = |root: &PathBuf| {
        resolved
            .strip_prefix(root)
            .is_ok_and(|rest| !rest.iter().any(|c| c.as_encoded_bytes().starts_with(b".")))
    };
    if !roots.iter().any(visible_below) {
        return Err(ToolError::invalid_args(format!(
            "{shown}: hidden folders and files (names starting with `.`) cannot be written"
        )));
    }
    match std::fs::symlink_metadata(&resolved) {
        Ok(m) if m.file_type().is_symlink() => Err(ToolError::invalid_args(format!(
            "{shown} is a symlink; export_file does not follow or replace symlinks"
        ))),
        Ok(m) if !m.is_file() => Err(ToolError::invalid_args(format!(
            "{shown} exists and is not a regular file"
        ))),
        Ok(_) => Ok(resolved),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(resolved),
        Err(e) => Err(ToolError::invalid_args(format!("{shown}: {e}"))),
    }
}

/// Write an export to `path`, a path [`check_export_path`] returned: missing
/// folders are created, the bytes go to a new temp file in the same folder,
/// and the temp file is renamed over `path`. A rename replaces a symlink or
/// hard link itself, never the file it points at.
pub fn write_export_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(std::io::Error::other(format!(
            "{} is not a file path",
            path.display()
        )));
    };
    std::fs::create_dir_all(dir)?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = dir.join(tmp_name);
    let write = || -> std::io::Result<()> {
        // `create_new` refuses anything already at the temp name, symlinks
        // included.
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    write().inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Canonicalize the longest existing ancestor of `path` and append the rest,
/// refusing any `..` in the part that does not exist yet.
fn resolve_existing_prefix(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while existing.canonicalize().is_err() {
        let name = existing.file_name()?.to_owned();
        rest.push(name);
        existing.pop();
    }
    let mut out = existing.canonicalize().ok()?;
    for name in rest.into_iter().rev() {
        if name == ".." || name == "." {
            return None;
        }
        out.push(name);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::symlink;

    /// A fresh `<temp>/god-agent-<name>-<pid>` with `docs`, `home` and
    /// `other` folders in it.
    fn temp_root(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("god-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let docs = root.join("docs");
        let home = root.join("home");
        let other = root.join("other");
        for d in [&docs, &home, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        (root, docs, home, other)
    }

    /// The `invalid_args` message for writing `format` to `path`.
    fn refused(path: &Path, format: ExportFormat, doc: &Path, home: &Path) -> String {
        let e = check_export_path(path, format, Some(doc), Some(home)).unwrap_err();
        assert_eq!(e.code, crate::ErrorCode::InvalidArgs, "{}", e.message);
        e.message
    }

    #[test]
    fn export_paths_stay_inside_the_document_dir_or_home() {
        let (root, docs, home, other) = temp_root("paths");
        symlink(&other, docs.join("link")).unwrap();
        let doc = docs.join("a.god.json");
        let allowed = |p: PathBuf| {
            let format = match p.extension().and_then(|e| e.to_str()) {
                Some("svg") => ExportFormat::Svg,
                _ => ExportFormat::Png,
            };
            check_export_path(&p, format, Some(&doc), Some(&home)).is_ok()
        };

        assert!(allowed(docs.join("out.png")));
        assert!(
            allowed(docs.join("new/dir/out.png")),
            "missing dirs are fine"
        );
        assert!(allowed(home.join("x.svg")));
        assert!(
            allowed(docs.join("../home/x.svg")),
            "existing `..` resolves"
        );
        assert!(!allowed(other.join("x.png")));
        assert!(!allowed(docs.join("../other/x.png")));
        assert!(!allowed(docs.join("link/x.png")), "symlinks resolve");
        assert!(!allowed(docs.join("new/../../other/x.png")));
        assert!(!allowed(PathBuf::from("relative.png")));
        assert!(check_export_path(&other.join("x.png"), ExportFormat::Png, None, None).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_extension_must_match_the_format() {
        let (root, docs, home, _) = temp_root("ext");
        let doc = docs.join("a.god.json");
        let check = |name: &str, f| check_export_path(&docs.join(name), f, Some(&doc), Some(&home));
        assert!(check("a.png", ExportFormat::Png).is_ok());
        assert!(check("a.PNG", ExportFormat::Png).is_ok(), "any case");
        assert!(check("a.Svg", ExportFormat::Svg).is_ok());
        for (name, f) in [
            ("a.svg", ExportFormat::Png),
            ("a.png", ExportFormat::Svg),
            ("a", ExportFormat::Png),
            ("a.png.txt", ExportFormat::Png),
            ("authorized_keys", ExportFormat::Svg),
        ] {
            let m = refused(&docs.join(name), f, &doc, &home);
            assert!(m.contains(&format!(".{} file", f.extension())), "{m}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dotfiles_are_refused() {
        let (root, docs, home, _) = temp_root("dotfile");
        let doc = docs.join("a.god.json");
        // `.png` alone has no extension: it is a dotfile named ".png".
        refused(&home.join(".png"), ExportFormat::Png, &doc, &home);
        let m = refused(&home.join(".x.png"), ExportFormat::Png, &doc, &home);
        assert!(m.contains("hidden"), "{m}");
        refused(&docs.join(".hidden.svg"), ExportFormat::Svg, &doc, &home);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hidden_folders_are_refused() {
        let (root, docs, home, _) = temp_root("hidden");
        let doc = docs.join("a.god.json");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        for p in [
            home.join(".ssh/authorized_keys.png"),
            home.join(".config/new/x.svg"),
            home.join("a/.b/c.png"),
            docs.join("sub/.git/x.png"),
            // `..` through an existing hidden folder still lands in it.
            home.join(".ssh/../.ssh/x.png"),
        ] {
            let f = if p.extension().unwrap() == "svg" {
                ExportFormat::Svg
            } else {
                ExportFormat::Png
            };
            let m = refused(&p, f, &doc, &home);
            assert!(m.contains("hidden"), "{}: {m}", p.display());
        }
        // A visible symlink into a hidden folder is resolved, then refused.
        symlink(home.join(".ssh"), home.join("keys")).unwrap();
        let m = refused(&home.join("keys/x.png"), ExportFormat::Png, &doc, &home);
        assert!(m.contains("hidden"), "{m}");
        // `..` out of a hidden folder that exists is fine.
        assert!(check_export_path(
            &home.join(".ssh/../ok.png"),
            ExportFormat::Png,
            Some(&doc),
            Some(&home)
        )
        .is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_document_folder_inside_a_hidden_folder_is_its_own_root() {
        let (root, _, home, _) = temp_root("hidden-doc");
        let docs = home.join(".work/designs");
        std::fs::create_dir_all(&docs).unwrap();
        let doc = docs.join("a.god.json");
        assert!(check_export_path(
            &docs.join("out.png"),
            ExportFormat::Png,
            Some(&doc),
            Some(&home)
        )
        .is_ok());
        refused(&docs.join(".out.png"), ExportFormat::Png, &doc, &home);
        refused(&home.join(".work/out.png"), ExportFormat::Png, &doc, &home);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_symlink_at_the_file_is_never_followed() {
        let (root, docs, home, other) = temp_root("symlink");
        let doc = docs.join("a.god.json");
        let target = other.join("victim.png");
        std::fs::write(&target, "keep").unwrap();
        symlink(&target, docs.join("out.png")).unwrap();
        let m = refused(&docs.join("out.png"), ExportFormat::Png, &doc, &home);
        assert!(m.contains("symlink"), "{m}");
        // A dangling one too.
        symlink(other.join("missing.png"), docs.join("dangling.png")).unwrap();
        refused(&docs.join("dangling.png"), ExportFormat::Png, &doc, &home);
        // Even one that points inside the allowed folder.
        std::fs::write(docs.join("real.png"), "x").unwrap();
        symlink(docs.join("real.png"), docs.join("alias.png")).unwrap();
        refused(&docs.join("alias.png"), ExportFormat::Png, &doc, &home);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_regular_files_are_overwritten() {
        let (root, docs, home, _) = temp_root("overwrite");
        let doc = docs.join("a.god.json");
        std::fs::write(docs.join("old.png"), "old").unwrap();
        let path = check_export_path(
            &docs.join("old.png"),
            ExportFormat::Png,
            Some(&doc),
            Some(&home),
        )
        .expect("a regular file with the same extension is replaced");
        write_export_file(&path, b"new").unwrap();
        assert_eq!(std::fs::read(docs.join("old.png")).unwrap(), b"new");

        std::fs::create_dir_all(docs.join("dir.svg")).unwrap();
        let m = refused(&docs.join("dir.svg"), ExportFormat::Svg, &doc, &home);
        assert!(m.contains("not a regular file"), "{m}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn writes_go_through_a_temp_file_and_rename() {
        let (root, docs, home, other) = temp_root("write");
        let doc = docs.join("a.god.json");
        let path = check_export_path(
            &docs.join("new/dir/out.svg"),
            ExportFormat::Svg,
            Some(&doc),
            Some(&home),
        )
        .unwrap();
        write_export_file(&path, b"<svg/>").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"<svg/>");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("out.svg")],
            "no temp file left"
        );

        // A symlink planted after the check is replaced, not written through.
        let target = other.join("victim.svg");
        std::fs::write(&target, "keep").unwrap();
        let racy = check_export_path(
            &docs.join("racy.svg"),
            ExportFormat::Svg,
            Some(&doc),
            Some(&home),
        )
        .unwrap();
        symlink(&target, &racy).unwrap();
        write_export_file(&racy, b"<svg/>").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert!(!std::fs::symlink_metadata(&racy)
            .unwrap()
            .file_type()
            .is_symlink());
        let _ = std::fs::remove_dir_all(&root);
    }
}
