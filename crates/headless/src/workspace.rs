//! `Workspace`: the agent's [`AppHandle`] over the design files in one
//! folder, with no window (§6.7). One document is open at a time; every
//! committed edit is saved to its file at once, so another program (or the
//! app) always reads the latest design. Nothing outside the folder is read
//! or written, except images from the folders given with `--images`.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use agent::{
    check_locked, write_export_file, AppHandle, DocumentCall, ExportFormat, ExportRequest,
    Notifier, Screenshot, ScreenshotRequest, SelectionState, Snapshot, ToolError, TxOk, TxRequest,
};
use doc::assets::assets_dir_for;
use doc::{Assets, Document, History, NodeId, Origin, Selection};
use layout::LayoutEngine;
use serde_json::{json, Value};

use crate::gpu::Gpu;
use crate::render::{render, Format, RenderOptions};

/// The suffix every design file has.
pub const DESIGN_SUFFIX: &str = ".tessel.json";
/// The largest image `import_image` takes.
pub const MAX_IMAGE_BYTES: u64 = 40 * 1024 * 1024;
/// `list_documents` stops after this many files.
const MAX_LISTED: usize = 500;

struct State {
    doc: Document,
    history: History,
    assets: Assets,
    /// The open file; `None` until `open_document` or `new_document`.
    path: Option<PathBuf>,
    selection: Vec<NodeId>,
    revision: u64,
}

pub struct Workspace {
    root: PathBuf,
    images: Vec<PathBuf>,
    /// `--templates`: read-only designs new_document can copy.
    templates: Option<PathBuf>,
    /// Families from `--fonts`.
    fonts: Vec<String>,
    state: Mutex<State>,
    published: Mutex<Arc<Snapshot>>,
    /// Made on the first render; `Err` is kept so a missing GPU is reported, not retried.
    gpu: Mutex<Option<Result<Gpu, String>>>,
    notifier: OnceLock<Notifier>,
}

thread_local! {
    // `LayoutEngine` is not `Send`: each thread keeps its own.
    static LAYOUT: std::cell::RefCell<LayoutEngine> = std::cell::RefCell::new(LayoutEngine::new());
}

fn layout(doc: &Document) -> layout::Geometry {
    LAYOUT.with(|l| l.borrow_mut().compute(doc))
}

impl Workspace {
    /// A workspace over `root` (which must exist), also reading images from
    /// `images`.
    pub fn new(root: &Path, images: &[PathBuf]) -> std::io::Result<Workspace> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} is not a folder",
                root.display()
            )));
        }
        let images = images
            .iter()
            .map(|d| d.canonicalize())
            .collect::<std::io::Result<Vec<_>>>()?;
        let state = State {
            doc: Document::default(),
            history: History::default(),
            assets: Assets::new(),
            path: None,
            selection: Vec::new(),
            revision: 0,
        };
        let ws = Workspace {
            root,
            images,
            templates: None,
            fonts: Vec::new(),
            published: Mutex::new(Arc::new(snapshot_of(&state))),
            state: Mutex::new(state),
            gpu: Mutex::new(None),
            notifier: OnceLock::new(),
        };
        Ok(ws)
    }

    /// Let new_document start from the `.tessel.json` files in `dir` (read
    /// only; outside the root is fine).
    pub fn with_templates(mut self, dir: &Path) -> std::io::Result<Workspace> {
        self.templates = Some(dir.canonicalize()?);
        Ok(self)
    }

    /// Make the fonts in `dir` available to this process's text (see
    /// `layout::text::add_fonts`). Call before the first layout.
    pub fn with_fonts(mut self, dir: &Path) -> std::io::Result<Workspace> {
        let added = layout::text::add_fonts(dir)?;
        self.fonts.extend(added);
        self.fonts.sort();
        self.fonts.dedup();
        Ok(self)
    }

    /// Wire up change notification (`Agent::notifier`).
    pub fn set_notifier(&self, n: Notifier) {
        let _ = self.notifier.set(n);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn publish(&self, s: &State) {
        *self.published.lock().unwrap() = Arc::new(snapshot_of(s));
    }

    fn changed(&self) {
        if let Some(n) = self.notifier.get() {
            n.document_changed();
        }
    }

    /// Render with the shared GPU, opening it on first use.
    fn with_gpu<T>(
        &self,
        f: impl FnOnce(&mut Gpu) -> Result<T, ToolError>,
    ) -> Result<T, ToolError> {
        let mut slot = self.gpu.lock().unwrap();
        let gpu = slot.get_or_insert_with(|| {
            Gpu::open()
                .inspect(|g| log::info!("rendering on {}", g.adapter))
                .map_err(|e| e.to_string())
        });
        match gpu {
            Ok(g) => f(g),
            Err(e) => Err(ToolError::internal(e.clone())),
        }
    }

    fn render(
        &self,
        node: &NodeId,
        format: Format,
        scale: f64,
        background: Option<String>,
    ) -> Result<crate::render::Rendered, ToolError> {
        let (doc, assets) = {
            let s = self.state.lock().unwrap();
            (s.doc.clone(), s.assets.clone())
        };
        let opts = RenderOptions {
            node: Some(node.to_string()),
            format,
            scale,
            background,
            ..RenderOptions::default()
        };
        let run = |gpu: Option<&mut Gpu>| {
            render(&doc, &assets, &opts, gpu).map_err(|e| ToolError::internal(e.to_string()))
        };
        match format {
            Format::Svg => run(None),
            _ => self.with_gpu(|g| run(Some(g))),
        }
    }

    // --- Paths -----------------------------------------------------------

    /// `path` (relative to the root, or absolute) as a file inside one of
    /// `bases`, with no hidden folder or file below the base. The file need
    /// not exist; its folder must.
    fn inside(&self, path: &str, bases: &[&Path]) -> Result<PathBuf, ToolError> {
        let given = Path::new(path);
        let joined = if given.is_absolute() {
            given.to_path_buf()
        } else {
            self.root.join(given)
        };
        let name = joined
            .file_name()
            .filter(|n| !n.to_string_lossy().starts_with('.'))
            .ok_or_else(|| ToolError::invalid_args(format!("{path}: not a file name")))?
            .to_owned();
        let parent = joined
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .ok_or_else(|| ToolError::invalid_args(format!("{path}: its folder does not exist")))?;
        let base = bases
            .iter()
            .find(|b| parent.starts_with(b))
            .ok_or_else(|| {
                ToolError::invalid_args(format!("{path} is outside the workspace folder"))
            })?;
        let hidden = parent
            .strip_prefix(base)
            .unwrap_or(Path::new(""))
            .components()
            .any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')));
        if hidden {
            return Err(ToolError::invalid_args(format!(
                "{path} is in a hidden folder"
            )));
        }
        let file = parent.join(name);
        // Never follow a symlink out of the folder.
        if file
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(ToolError::invalid_args(format!("{path} is a symlink")));
        }
        Ok(file)
    }

    fn design_path(&self, path: &str) -> Result<PathBuf, ToolError> {
        if !path.ends_with(DESIGN_SUFFIX) {
            return Err(ToolError::invalid_args(format!(
                "{path}: a design file ends in {DESIGN_SUFFIX}"
            )));
        }
        self.inside(path, &[&self.root])
    }

    fn shown(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .unwrap_or(p)
            .display()
            .to_string()
    }

    // --- Document tools --------------------------------------------------

    fn list(&self) -> Value {
        let mut found = Vec::new();
        let mut dirs = vec![self.root.clone()];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Ok(kind) = e.file_type() else { continue };
                if name.starts_with('.') || kind.is_symlink() {
                    continue;
                }
                if kind.is_dir() {
                    dirs.push(e.path());
                } else if name.ends_with(DESIGN_SUFFIX) && found.len() < MAX_LISTED {
                    found.push(e.path());
                }
            }
        }
        found.sort();
        let docs: Vec<Value> = found
            .iter()
            .map(|p| {
                let title = doc::load_file(p)
                    .ok()
                    .map(|l| l.doc.meta.title)
                    .unwrap_or_default();
                json!({ "path": self.shown(p), "title": title })
            })
            .collect();
        json!({ "root": self.root, "documents": docs, "templates": self.template_list(), "fonts": self.fonts })
    }

    fn template_list(&self) -> Vec<Value> {
        let Some(dir) = &self.templates else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(DESIGN_SUFFIX))
            .collect();
        files.sort();
        files
            .iter()
            .filter_map(|p| {
                let name = p.file_name()?.to_string_lossy();
                let name = name.strip_suffix(DESIGN_SUFFIX)?.to_string();
                let loaded = doc::load_file(p).ok()?;
                let meta = &loaded.doc.meta;
                let frame = loaded.doc.pages.first()?.children.first()?;
                Some(json!({
                    "name": name,
                    "title": meta.title,
                    "description": meta.extra.get("description").and_then(Value::as_str).unwrap_or_default(),
                    "width": frame.width,
                    "height": frame.height,
                }))
            })
            .collect()
    }

    /// The template file called `name`.
    fn template_file(&self, name: &str) -> Result<PathBuf, ToolError> {
        let Some(dir) = &self.templates else {
            return Err(ToolError::invalid_args(
                "this workspace has no templates (start the server with --templates)",
            ));
        };
        let plain = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let file = dir.join(format!("{name}{DESIGN_SUFFIX}"));
        match file.symlink_metadata() {
            Ok(m) if plain && m.is_file() => Ok(file),
            _ => Err(ToolError::invalid_args(format!(
                "no template {name:?}; list_documents lists them"
            ))),
        }
    }

    /// Create the folders `path` names inside the root ("acme/sale.tessel.json"
    /// makes `acme/`). Only plain names: no `..`, no hidden folder.
    fn make_folders(&self, path: &str) -> Result<(), ToolError> {
        let given = Path::new(path);
        let relative = match given.strip_prefix(&self.root) {
            Ok(rest) => rest,
            Err(_) if given.is_absolute() => return Ok(()),
            Err(_) => given,
        };
        let Some(folders) = relative.parent() else {
            return Ok(());
        };
        let plain = folders
            .components()
            .all(|c| matches!(c, Component::Normal(n) if !n.to_string_lossy().starts_with('.')));
        if !plain {
            return Ok(()); // design_path explains what is wrong with it
        }
        // One level at a time, never through a symlink out of the root.
        let mut dir = self.root.clone();
        for part in folders.components() {
            dir.push(part);
            match dir.symlink_metadata() {
                Ok(m) if m.is_dir() => continue,
                Ok(_) => return Ok(()),
                Err(_) => std::fs::create_dir(&dir)
                    .map_err(|e| ToolError::internal(format!("{path}: {e}")))?,
            }
        }
        Ok(())
    }

    fn open(&self, path: &str) -> Result<Value, ToolError> {
        let file = self.design_path(path)?;
        let loaded =
            doc::load_file(&file).map_err(|e| ToolError::invalid_args(format!("{path}: {e}")))?;
        let assets = Assets::new();
        let dir = assets_dir_for(&file);
        assets.set_dir(Some(dir.clone()));
        assets.load_dir(&dir, &loaded.doc);
        let mut s = self.state.lock().unwrap();
        s.doc = loaded.doc;
        s.history = History::default();
        s.assets = assets;
        s.path = Some(file.clone());
        s.selection.clear();
        // A different document: an `if_revision` from the last one must fail.
        s.revision += 1;
        self.publish(&s);
        drop(s);
        self.changed();
        Ok(self.opened(&file))
    }

    /// What `open_document` and `new_document` return: the file and its frames.
    fn opened(&self, file: &Path) -> Value {
        let s = self.state.lock().unwrap();
        let pages: Vec<Value> = s
            .doc
            .pages
            .iter()
            .map(|p| {
                let frames: Vec<Value> = p
                    .children
                    .iter()
                    .map(|n| {
                        json!({ "id": n.id.to_string(), "name": n.name, "kind": n.kind.tag(),
                                     "width": n.width, "height": n.height })
                    })
                    .collect();
                json!({ "id": p.id.to_string(), "name": p.name, "children": frames })
            })
            .collect();
        json!({ "path": self.shown(file), "title": s.doc.meta.title, "revision": s.revision, "pages": pages })
    }

    fn new_document(
        &self,
        path: &str,
        template: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
        title: Option<String>,
        background: Option<String>,
    ) -> Result<Value, ToolError> {
        let source = match &template {
            Some(name) => {
                if width.is_some() || height.is_some() || background.is_some() {
                    return Err(ToolError::invalid_args(
                        "a template has its own size and background; leave out width, height and background",
                    ));
                }
                Some(self.template_file(name)?)
            }
            None => None,
        };
        self.make_folders(path)?;
        let file = self.design_path(path)?;
        if file.symlink_metadata().is_ok() {
            return Err(ToolError::invalid_args(format!(
                "{path} already exists; open it instead"
            )));
        }
        let default_title = || {
            file.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .trim_end_matches(DESIGN_SUFFIX)
                .to_string()
        };
        if let Some(source) = source {
            let mut loaded = doc::load_file(&source)
                .map_err(|e| ToolError::internal(format!("template: {e}")))?;
            loaded.doc.meta.title = title.unwrap_or_else(default_title);
            loaded.doc.meta.extra.remove("description");
            // The template's images go with it.
            let assets = Assets::new();
            assets.load_dir(&assets_dir_for(&source), &loaded.doc);
            assets
                .save(&assets_dir_for(&file), &loaded.doc)
                .and_then(|_| doc::save_file(&file, &loaded.doc))
                .map_err(|e| ToolError::internal(format!("{path}: {e}")))?;
            return self.open(path);
        }
        let (Some(width), Some(height)) = (width, height) else {
            return Err(ToolError::invalid_args(
                "give a template, or width and height for a blank design",
            ));
        };
        let ok = |v: f32| v.is_finite() && (1.0..=20_000.0).contains(&v);
        if !ok(width) || !ok(height) {
            return Err(ToolError::invalid_args(
                "width and height must be between 1 and 20000",
            ));
        }
        let background = background.unwrap_or_else(|| "#ffffff".into());
        if render::parse_hex(&background).is_none() {
            return Err(ToolError::invalid_args(
                "background is not a #rrggbb[aa] color",
            ));
        }
        let title = title.unwrap_or_else(default_title);
        let source = json!({
            "format": { "name": "tessel", "version": 1 },
            "meta": { "title": title },
            "pages": [{ "id": doc::Id::new(doc::IdKind::Page).to_string(), "name": "Page 1", "children": [{
                "id": doc::Id::new(doc::IdKind::Node).to_string(), "name": "Design", "kind": "frame",
                "x": 0, "y": 0, "width": width, "height": height,
                "fills": [{ "type": "solid", "color": background }]
            }]}]
        });
        let loaded =
            doc::load_str(&source.to_string()).map_err(|e| ToolError::internal(e.to_string()))?;
        doc::save_file(&file, &loaded.doc)
            .map_err(|e| ToolError::internal(format!("{path}: {e}")))?;
        self.open(path)
    }

    fn import_image(&self, path: &str) -> Result<Value, ToolError> {
        let mut bases: Vec<&Path> = vec![&self.root];
        bases.extend(self.images.iter().map(PathBuf::as_path));
        let file = self.inside(path, &bases)?;
        if self.state.lock().unwrap().path.is_none() {
            return Err(no_document());
        }
        let meta = std::fs::metadata(&file)
            .map_err(|e| ToolError::invalid_args(format!("{path}: {e}")))?;
        if !meta.is_file() || meta.len() > MAX_IMAGE_BYTES {
            return Err(ToolError::invalid_args(format!(
                "{path}: not an image file of at most 40 MB"
            )));
        }
        let bytes =
            std::fs::read(&file).map_err(|e| ToolError::invalid_args(format!("{path}: {e}")))?;
        let (width, height) = render::natural_size(&bytes).ok_or_else(|| {
            ToolError::invalid_args(format!("{path}: not a PNG, JPEG, GIF or WebP image"))
        })?;
        let ext = file
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        // Kept in memory until a layer uses it; the save after that edit writes it.
        let asset = self.state.lock().unwrap().assets.add(bytes, &ext);
        Ok(json!({ "asset": asset.0, "width": width, "height": height }))
    }

    fn save(&self, s: &State) -> Result<(), ToolError> {
        let Some(path) = &s.path else {
            return Err(no_document());
        };
        // Assets first: a document that names a missing image is worse than an unused image.
        s.assets
            .save(&assets_dir_for(path), &s.doc)
            .and_then(|_| doc::save_file(path, &s.doc))
            .map_err(|e| ToolError::internal(format!("saving {}: {e}", self.shown(path))))
    }
}

fn no_document() -> ToolError {
    ToolError::invalid_args("no document is open: call open_document or new_document first")
}

fn snapshot_of(s: &State) -> Snapshot {
    Snapshot {
        document: s.doc.clone(),
        geometry: layout(&s.doc),
        selection: SelectionState {
            page_id: s.doc.pages.first().map(|p| p.id.clone()),
            ids: s.selection.clone(),
        },
        revision: s.revision,
        dirty: false,
        path: s.path.clone(),
        assets: s.assets.clone(),
    }
}

impl AppHandle for Workspace {
    fn snapshot(&self) -> Arc<Snapshot> {
        self.published.lock().unwrap().clone()
    }

    async fn submit(&self, tx: TxRequest) -> Result<TxOk, ToolError> {
        let mut s = self.state.lock().unwrap();
        if s.path.is_none() {
            return Err(no_document());
        }
        if let Some(expected) = tx.if_revision {
            if expected != s.revision {
                return Err(ToolError::stale_revision(expected, s.revision));
            }
        }
        check_locked(&s.doc, &tx.commands)?;
        let touched: BTreeSet<NodeId> = tx.commands.iter().flat_map(|c| c.touched_ids()).collect();
        let before = Selection {
            ids: s.selection.clone(),
        };
        let after = Selection {
            ids: tx.select.clone().unwrap_or_else(|| s.selection.clone()),
        };
        let State { doc, history, .. } = &mut *s;
        history.commit(
            doc,
            tx.label,
            Origin::Agent { client: tx.client },
            tx.commands,
            before,
            after.clone(),
        )?;
        s.selection = after.ids;
        s.revision += 1;
        self.save(&s)?;
        self.publish(&s);
        let revision = s.revision;
        drop(s);
        self.changed();
        Ok(TxOk { revision, touched })
    }

    async fn set_selection(
        &self,
        ids: Vec<NodeId>,
        _zoom_to: bool,
    ) -> Result<Vec<NodeId>, ToolError> {
        let mut s = self.state.lock().unwrap();
        s.selection = ids.clone();
        self.publish(&s);
        Ok(ids)
    }

    async fn screenshot(&self, req: ScreenshotRequest) -> Result<Screenshot, ToolError> {
        let g = self
            .snapshot()
            .geometry
            .get(&req.node)
            .cloned()
            .ok_or_else(|| ToolError::not_found(&req.node))?;
        let longest = g.bounds.width().max(g.bounds.height()) as f32;
        let scale = req.scale.min(req.max_size as f32 / longest.max(1.0));
        let shot = self.render(
            &req.node,
            Format::Png,
            scale as f64,
            req.background.map(|c| c.0),
        )?;
        Ok(Screenshot {
            png: shot.bytes,
            width: shot.layers.width,
            height: shot.layers.height,
            scale,
        })
    }

    async fn export_file(&self, req: ExportRequest) -> Result<u64, ToolError> {
        // The tool allowed the document's folder or $HOME; here only the workspace.
        let path = self.inside(&req.path.to_string_lossy(), &[&self.root])?;
        let format = match req.format {
            ExportFormat::Png => Format::Png,
            ExportFormat::Svg => Format::Svg,
        };
        let bytes = self
            .render(&req.node, format, req.scale as f64, None)?
            .bytes;
        write_export_file(&path, &bytes)
            .map_err(|e| ToolError::internal(format!("{}: {e}", self.shown(&path))))?;
        Ok(bytes.len() as u64)
    }

    fn has_documents(&self) -> bool {
        true
    }

    async fn documents(&self, call: DocumentCall) -> Result<Value, ToolError> {
        match call {
            DocumentCall::List => Ok(self.list()),
            DocumentCall::Open { path } => self.open(&path),
            DocumentCall::New {
                path,
                template,
                width,
                height,
                title,
                background,
            } => self.new_document(&path, template, width, height, title, background),
            DocumentCall::ImportImage { path } => self.import_image(&path),
        }
    }
}
