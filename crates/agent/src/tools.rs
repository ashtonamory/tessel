//! The MCP tools (§6.3): what each one reads, the commands it builds and
//! what it returns. Every mutating tool is one transaction (§5.3).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use doc::resolve::detach_style_patch;
use doc::{
    Color, Command, Document, Id, IdKind, Node, NodeId, NodeKind, NodePatch, ParentRef, Style,
    StyleKind, TokenId, TokensPatch,
};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};

use crate::args::*;
use crate::error::ToolError;
use crate::handle::{
    check_export_path, check_locked, AppHandle, DocumentCall, ExportFormat, ExportRequest,
    ScreenshotRequest, Snapshot, TxOk, TxRequest,
};
use crate::json::{
    build_node, children_value, file_json, node_json, refresh_ids, round2, summary, summary_tree,
};

/// What a successful tool call returns.
#[derive(Debug, PartialEq)]
pub enum ToolOutput {
    Json(Value),
    /// A PNG plus a JSON description of it.
    Image {
        png: Vec<u8>,
        info: Value,
    },
}

struct ToolDef {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Arc<JsonObject>,
    /// Changes the document (refused in read-only mode).
    mutating: bool,
}

fn def<T: JsonSchema + 'static>(
    name: &'static str,
    mutating: bool,
    description: &'static str,
) -> ToolDef {
    ToolDef {
        name,
        description,
        schema: schema_for_type::<T>,
        mutating,
    }
}

fn defs() -> Vec<ToolDef> {
    vec![
        def::<GetDocumentArgs>(
            "get_document",
            false,
            "Summary of the open document: revision, title, path, dirty, pages with \
             their layer trees (summarised below `depth`), tokens, styles and components.",
        ),
        def::<GetNodeArgs>(
            "get_node",
            false,
            "One node in file-format JSON (what you can write back with update_node), \
             its absolute page-space geometry, parent, index and name path.",
        ),
        def::<FindNodesArgs>(
            "find_nodes",
            false,
            "Find nodes by name substring, kind, text content and/or ancestor.",
        ),
        def::<EmptyArgs>(
            "get_selection",
            false,
            "What the user has selected right now and the top-level frame it is in. \
             Call this when the user says \"this\" or \"the selected …\".",
        ),
        def::<EmptyArgs>(
            "get_tokens",
            false,
            "All design tokens and styles in file-format JSON.",
        ),
        def::<CreateNodeArgs>(
            "create_node",
            true,
            "Create a node, or a whole subtree via nested `children`, in one undo step. \
             Returns the root `id` and `ids`, mapping each created node's name path \
             (\"Card/Title\") to its ID.",
        ),
        def::<UpdateNodeArgs>(
            "update_node",
            true,
            "Change any props of one node with a partial file-format JSON patch.",
        ),
        def::<UpdateNodesArgs>(
            "update_nodes",
            true,
            "Patch several nodes in one undo step.",
        ),
        def::<DeleteNodesArgs>("delete_nodes", true, "Delete nodes and their subtrees."),
        def::<MoveNodeArgs>(
            "move_node",
            true,
            "Reparent and/or reorder and/or reposition a node.",
        ),
        def::<DuplicateNodeArgs>(
            "duplicate_node",
            true,
            "Copy a node and its subtree with new IDs.",
        ),
        def::<SetTokensArgs>(
            "set_tokens",
            true,
            "Add, change or remove (`null`) color, spacing, radius and font tokens. \
             Reference them in props as \"{color.primary}\".",
        ),
        def::<UpsertStyleArgs>(
            "upsert_style",
            true,
            "Create a named fill, stroke, text or effect style, or update one by `id`.",
        ),
        def::<ApplyStyleArgs>(
            "apply_style",
            true,
            "Apply a style to nodes, or detach it (`style_id: null`).",
        ),
        def::<CreateComponentArgs>(
            "create_component",
            true,
            "Turn a frame into a component. The frame stays in place as an instance \
             with the same ID (`instance_id`); the master is the new component.",
        ),
        def::<CreateInstanceArgs>(
            "create_instance",
            true,
            "Place an instance of a component, optionally with overrides keyed by \
             component node ID.",
        ),
        def::<SetSelectionArgs>(
            "set_selection",
            false,
            "Select nodes in the app to point the user at them. Not an edit; not in history.",
        ),
        def::<ScreenshotArgs>(
            "screenshot",
            false,
            "Render a node exactly as the canvas shows it, as a PNG. Use it after edits \
             to check your work.",
        ),
        def::<ExportCodeArgs>(
            "export_code",
            false,
            "Export a node as HTML + CSS, Tailwind or React code. Returns the files \
             (`path`, `language`, `content`; image files are base64 with \
             `encoding: \"base64\"`). Nothing is written to disk.",
        ),
        def::<ExportFileArgs>(
            "export_file",
            false,
            "Export a node to a PNG or SVG file. The path must end in .png or .svg to \
             match `format`, and be inside the document's directory or $HOME, with no \
             hidden folder or file (a name starting with `.`) below that folder. A \
             symlink is never followed or replaced, and an existing file is only \
             overwritten if it is a regular file. Relative paths are relative to the \
             document's directory.",
        ),
    ]
}

/// The document tools: headless mode only (§6.7). Opening or creating a
/// document is not an edit of it, so none is refused in read-only mode.
fn document_defs() -> Vec<ToolDef> {
    vec![
        def::<EmptyArgs>(
            "list_documents",
            false,
            "The .tessel.json files in the workspace folder, with their titles.",
        ),
        def::<OpenDocumentArgs>(
            "open_document",
            false,
            "Open a .tessel.json file from the workspace folder. Every other tool then \
             works on it, and each edit is saved to it at once.",
        ),
        def::<NewDocumentArgs>(
            "new_document",
            false,
            "Create and open a new .tessel.json file with one page and one frame of the \
             given size (\"Design\"). Returns the page and frame IDs to build in.",
        ),
        def::<ImportImageArgs>(
            "import_image",
            false,
            "Add a photo or logo to the open document. Returns its `asset` and natural \
             size; place it with create_node (kind \"image\", `asset`) or as an image fill.",
        ),
    ]
}

/// The tools for `tools/list` in the app.
pub fn list() -> Vec<Tool> {
    list_for(false)
}

/// The tools for `tools/list`, with the document tools when the handle has
/// them.
pub fn list_for(documents: bool) -> Vec<Tool> {
    let extra = if documents {
        document_defs()
    } else {
        Vec::new()
    };
    defs()
        .into_iter()
        .chain(extra)
        .map(|d| {
            let annotations = if d.mutating {
                ToolAnnotations::new().read_only(false).destructive(false)
            } else {
                ToolAnnotations::new().read_only(!matches!(
                    d.name,
                    "export_file" | "new_document" | "import_image"
                ))
            };
            Tool::new(d.name, d.description, (d.schema)()).with_annotations(annotations)
        })
        .collect()
}

/// Run a tool. `None` if there is no tool called `name`.
pub async fn call<H: AppHandle>(
    app: &H,
    client: &str,
    read_only: bool,
    name: &str,
    args: Value,
) -> Option<Result<ToolOutput, ToolError>> {
    if app.has_documents() && document_defs().iter().any(|d| d.name == name) {
        let call = match name {
            "list_documents" => parse::<EmptyArgs>(args).map(|_| DocumentCall::List),
            "open_document" => {
                parse::<OpenDocumentArgs>(args).map(|a| DocumentCall::Open { path: a.path })
            }
            "new_document" => parse::<NewDocumentArgs>(args).map(|a| DocumentCall::New {
                path: a.path,
                width: a.width,
                height: a.height,
                title: a.title,
                background: a.background,
            }),
            "import_image" => {
                parse::<ImportImageArgs>(args).map(|a| DocumentCall::ImportImage { path: a.path })
            }
            _ => unreachable!("every document tool is dispatched"),
        };
        return Some(match call {
            Ok(call) => app.documents(call).await.map(ToolOutput::Json),
            Err(e) => Err(e),
        });
    }
    let d = defs().into_iter().find(|d| d.name == name)?;
    if d.mutating && read_only {
        return Some(Err(ToolError::read_only()));
    }
    let cx = Cx {
        app,
        client,
        snap: app.snapshot(),
    };
    // Mutating and look/export tools are async; reads are not.
    macro_rules! run {
        ($f:ident) => {
            match parse(args) {
                Ok(a) => cx.$f(a).await,
                Err(e) => Err(e),
            }
        };
    }
    let out = match name {
        "get_document" => parse(args).and_then(|a| cx.get_document(a)),
        "get_node" => parse(args).and_then(|a| cx.get_node(a)),
        "find_nodes" => parse(args).and_then(|a| cx.find_nodes(a)),
        "get_selection" => parse::<EmptyArgs>(args).and_then(|_| cx.get_selection()),
        "get_tokens" => parse::<EmptyArgs>(args).map(|_| cx.get_tokens()),
        "create_node" => run!(create_node),
        "update_node" => run!(update_node),
        "update_nodes" => run!(update_nodes),
        "delete_nodes" => run!(delete_nodes),
        "move_node" => run!(move_node),
        "duplicate_node" => run!(duplicate_node),
        "set_tokens" => run!(set_tokens),
        "upsert_style" => run!(upsert_style),
        "apply_style" => run!(apply_style),
        "create_component" => run!(create_component),
        "create_instance" => run!(create_instance),
        "export_code" => parse(args).and_then(|a| cx.export_code(a)),
        "set_selection" => run!(set_selection),
        "screenshot" => run!(screenshot),
        "export_file" => run!(export_file),
        _ => unreachable!("every listed tool is dispatched"),
    };
    Some(out)
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| ToolError::invalid_args(e.to_string()))
}

fn ok(v: Value) -> Result<ToolOutput, ToolError> {
    Ok(ToolOutput::Json(v))
}

fn node_id(s: &str) -> NodeId {
    Id::from_raw(s)
}

/// A name for labels: the layer name, or its kind.
fn label_name(n: &Node) -> String {
    if n.name.is_empty() {
        n.kind.tag().to_string()
    } else {
        n.name.clone()
    }
}

fn layers_label(verb: &str, doc: &Document, ids: &[NodeId]) -> String {
    match ids {
        [one] => format!(
            "Agent: {verb} {}",
            doc.node(one).map(label_name).unwrap_or_default()
        ),
        many => format!("Agent: {verb} {} layers", many.len()),
    }
}

/// One tool call against one snapshot.
struct Cx<'a, H> {
    app: &'a H,
    client: &'a str,
    snap: Arc<Snapshot>,
}

impl<H: AppHandle> Cx<'_, H> {
    fn doc(&self) -> &Document {
        &self.snap.document
    }

    fn node(&self, id: &str) -> Result<&Node, ToolError> {
        self.doc()
            .node(&node_id(id))
            .ok_or_else(|| ToolError::not_found(id))
    }

    /// A page or container node to put children in.
    fn parent_ref(&self, id: &str) -> Result<ParentRef, ToolError> {
        let id = node_id(id);
        if self.doc().page(&id).is_some() {
            Ok(ParentRef::Page(id))
        } else if self.doc().node(&id).is_some() {
            Ok(ParentRef::Node(id))
        } else {
            Err(ToolError::not_found(id))
        }
    }

    async fn commit(
        &self,
        commands: Vec<Command>,
        label: String,
        if_revision: Option<u64>,
        select: Option<Vec<NodeId>>,
    ) -> Result<TxOk, ToolError> {
        check_locked(self.doc(), &commands)?;
        self.app
            .submit(TxRequest {
                commands,
                label,
                client: self.client.to_string(),
                if_revision,
                select,
            })
            .await
    }

    // -- Read ---------------------------------------------------------------

    fn get_document(&self, a: GetDocumentArgs) -> Result<ToolOutput, ToolError> {
        document_json(&self.snap, a.page_id.as_deref(), a.depth).map(ToolOutput::Json)
    }

    fn get_node(&self, a: GetNodeArgs) -> Result<ToolOutput, ToolError> {
        let doc = self.doc();
        let node = self.node(&a.id)?;
        let (parent, index) = doc.parent_of(&node.id).expect("indexed node has a parent");
        let path = name_path(doc, &node.id);
        let mut out = Map::new();
        out.insert("node".into(), node_json(node, a.depth));
        if a.include_geometry {
            if let Some(g) = self.snap.geometry.get(&node.id) {
                let o = g.origin();
                let [m0, m1, ..] = g.transform.as_coeffs();
                out.insert(
                    "geometry".into(),
                    json!({
                        "x": round2(o.x),
                        "y": round2(o.y),
                        "width": round2(g.size.width),
                        "height": round2(g.size.height),
                        "rotation": round2(m1.atan2(m0).to_degrees()),
                    }),
                );
            }
        }
        out.insert("parent_id".into(), json!(parent_id(&parent)));
        out.insert("index".into(), json!(index));
        out.insert("path".into(), json!(path));
        ok(Value::Object(out))
    }

    fn find_nodes(&self, a: FindNodesArgs) -> Result<ToolOutput, ToolError> {
        let doc = self.doc();
        let roots: Vec<&Node> = match &a.within {
            None => doc.pages.iter().flat_map(|p| &p.children).collect(),
            Some(id) => match doc.page(&node_id(id)) {
                Some(p) => p.children.iter().collect(),
                None => self.node(id)?.children().into_iter().flatten().collect(),
            },
        };
        let name = a.name.map(|s| s.to_lowercase());
        let text = a.text.map(|s| s.to_lowercase());
        let mut results = Vec::new();
        let limit = a.limit as usize;
        for root in roots {
            root.walk(&mut |n| {
                if results.len() >= limit {
                    return;
                }
                let name_ok = name
                    .as_ref()
                    .is_none_or(|s| n.name.to_lowercase().contains(s));
                let kind_ok = a.kind.is_none_or(|k| k.tag() == n.kind.tag());
                let text_ok = text.as_ref().is_none_or(|s| match &n.kind {
                    NodeKind::Text(t) => t.content.to_lowercase().contains(s),
                    _ => false,
                });
                if name_ok && kind_ok && text_ok {
                    results.push(summary(n, &self.snap.geometry));
                }
            });
        }
        ok(json!({ "results": results }))
    }

    fn get_selection(&self) -> Result<ToolOutput, ToolError> {
        ok(selection_json(&self.snap))
    }

    fn get_tokens(&self) -> ToolOutput {
        ToolOutput::Json(json!({
            "tokens": file_json(&self.doc().tokens),
            "styles": file_json(&self.doc().styles),
        }))
    }

    // -- Write --------------------------------------------------------------

    async fn create_node(&self, a: CreateNodeArgs) -> Result<ToolOutput, ToolError> {
        let parent = self.parent_ref(&a.parent_id)?;
        let built = build_node(a.node)?;
        let id = built.node.id.clone();
        let label = format!("Agent: create {}", label_name(&built.node));
        let commands = vec![Command::CreateNode {
            parent,
            index: a.index,
            node: built.node,
        }];
        let select = a.select.then(|| vec![id.clone()]);
        let tx = self.commit(commands, label, a.if_revision, select).await?;
        ok(json!({ "revision": tx.revision, "id": id, "ids": built.ids }))
    }

    /// A `SetProps` for `id`, checked against the node so errors name it.
    fn patch_command(&self, id: &str, patch: Map<String, Value>) -> Result<Command, ToolError> {
        let node = self.node(id)?;
        let patch = NodePatch::from_json(Value::Object(patch))
            .map_err(|e| ToolError::from(e).with_node(id))?;
        patch
            .clone()
            .apply_to(&mut node.clone())
            .map_err(|e| ToolError::from(e).with_node(id))?;
        Ok(Command::SetProps {
            id: node.id.clone(),
            patch,
        })
    }

    async fn update_node(&self, a: UpdateNodeArgs) -> Result<ToolOutput, ToolError> {
        let cmd = self.patch_command(&a.id, a.patch)?;
        let id = node_id(&a.id);
        let label = layers_label("update", self.doc(), std::slice::from_ref(&id));
        let tx = self.commit(vec![cmd], label, a.if_revision, None).await?;
        ok(json!({ "revision": tx.revision, "id": id }))
    }

    async fn update_nodes(&self, a: UpdateNodesArgs) -> Result<ToolOutput, ToolError> {
        if a.updates.is_empty() {
            return Err(ToolError::invalid_args("updates is empty"));
        }
        let mut commands = Vec::new();
        let mut ids = Vec::new();
        for u in a.updates {
            commands.push(self.patch_command(&u.id, u.patch)?);
            ids.push(node_id(&u.id));
        }
        let label = layers_label("update", self.doc(), &ids);
        let tx = self.commit(commands, label, a.if_revision, None).await?;
        ok(json!({ "revision": tx.revision, "ids": ids }))
    }

    async fn delete_nodes(&self, a: DeleteNodesArgs) -> Result<ToolOutput, ToolError> {
        if a.ids.is_empty() {
            return Err(ToolError::invalid_args("ids is empty"));
        }
        let mut ids: Vec<NodeId> = Vec::new();
        for id in &a.ids {
            let id = self.node(id)?.id.clone();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        // Deleting a node deletes its subtree; skip IDs inside another.
        let doc = self.doc();
        let commands = ids
            .iter()
            .filter(|id| {
                !ids.iter()
                    .any(|other| other != *id && doc.node(other).is_some_and(|n| n.contains(id)))
            })
            .map(|id| Command::DeleteNode { id: id.clone() })
            .collect();
        let label = layers_label("delete", doc, &ids);
        let tx = self.commit(commands, label, a.if_revision, None).await?;
        ok(json!({ "revision": tx.revision, "deleted": ids }))
    }

    async fn move_node(&self, a: MoveNodeArgs) -> Result<ToolOutput, ToolError> {
        let node = self.node(&a.id)?;
        let (current, _) = self.doc().parent_of(&node.id).expect("indexed");
        let mut commands = Vec::new();
        let parent = match &a.parent_id {
            Some(p) => self.parent_ref(p)?,
            None => current.clone(),
        };
        if parent != current || a.index.is_some() {
            commands.push(Command::MoveNode {
                id: node.id.clone(),
                parent,
                index: a.index,
            });
        }
        if a.x.is_some() || a.y.is_some() {
            commands.push(Command::SetProps {
                id: node.id.clone(),
                patch: NodePatch {
                    x: a.x,
                    y: a.y,
                    ..NodePatch::default()
                },
            });
        }
        if commands.is_empty() {
            return Err(ToolError::invalid_args(
                "nothing to do: give parent_id, index, x or y",
            ));
        }
        let label = format!("Agent: move {}", label_name(node));
        let tx = self.commit(commands, label, a.if_revision, None).await?;
        ok(json!({ "revision": tx.revision, "id": node.id }))
    }

    async fn duplicate_node(&self, a: DuplicateNodeArgs) -> Result<ToolOutput, ToolError> {
        let orig = self.node(&a.id)?;
        let (current, at) = self.doc().parent_of(&orig.id).expect("indexed");
        let parent = match &a.parent_id {
            Some(p) => self.parent_ref(p)?,
            None => current.clone(),
        };
        let index = a.index.or_else(|| (parent == current).then_some(at + 1));
        let mut copy = orig.clone();
        refresh_ids(&mut copy);
        let [dx, dy] = a.offset.unwrap_or([0.0, 0.0]);
        copy.x += dx;
        copy.y += dy;
        let id = copy.id.clone();
        let label = format!("Agent: duplicate {}", label_name(orig));
        let commands = vec![Command::CreateNode {
            parent,
            index,
            node: copy,
        }];
        let tx = self.commit(commands, label, a.if_revision, None).await?;
        ok(json!({ "revision": tx.revision, "id": id }))
    }

    async fn create_component(&self, a: CreateComponentArgs) -> Result<ToolOutput, ToolError> {
        let from = self.node(&a.from_id)?;
        let instance_id = from.id.clone();
        let label = format!("Agent: create component {}", label_name(from));
        // The command's IDs depend only on the document, so apply it to a
        // copy to learn the component's ID (redo makes the same ones).
        let create = Command::CreateComponent {
            from: instance_id.clone(),
        };
        let mut copy = self.doc().clone();
        create
            .clone()
            .apply(&mut copy)
            .map_err(|e| ToolError::from(e).with_node(&a.from_id))?;
        let component = copy
            .node(&instance_id)
            .and_then(|n| match &n.kind {
                NodeKind::Instance(i) => doc::expand::component(&copy, &i.component),
                _ => None,
            })
            .expect("the frame became an instance")
            .clone();
        let component_id = component.id.clone();
        let mut commands = vec![create];
        if let Some(name) = a.name.filter(|n| *n != component.name) {
            commands.push(Command::UpsertComponent {
                component: doc::Component { name, ..component },
            });
        }
        let tx = self.commit(commands, label, a.if_revision, None).await?;
        ok(json!({
            "revision": tx.revision,
            "component_id": component_id,
            "instance_id": instance_id,
        }))
    }

    async fn create_instance(&self, a: CreateInstanceArgs) -> Result<ToolOutput, ToolError> {
        let parent = self.parent_ref(&a.parent_id)?;
        let comp = doc::expand::component(self.doc(), &node_id(&a.component_id))
            .ok_or_else(|| ToolError::not_found(&a.component_id))?;
        let mut spec = Map::new();
        spec.insert("kind".into(), json!("instance"));
        spec.insert("name".into(), json!(comp.name));
        spec.insert("component".into(), json!(comp.id));
        spec.insert("x".into(), json!(a.x.unwrap_or(0.0)));
        spec.insert("y".into(), json!(a.y.unwrap_or(0.0)));
        spec.insert("width".into(), json!(comp.root.width));
        spec.insert("height".into(), json!(comp.root.height));
        if let Some(o) = a.overrides {
            spec.insert("overrides".into(), Value::Object(o));
        }
        let built = build_node(spec)?;
        let id = built.node.id.clone();
        let label = format!("Agent: create instance of {}", comp.name);
        let commands = vec![Command::CreateNode {
            parent,
            index: a.index,
            node: built.node,
        }];
        let select = a.select.then(|| vec![id.clone()]);
        let tx = self.commit(commands, label, a.if_revision, select).await?;
        ok(json!({ "revision": tx.revision, "id": id }))
    }

    fn export_code(&self, a: ExportCodeArgs) -> Result<ToolOutput, ToolError> {
        use export::{CodeFormat, CodeOptions, FileContent, ImageMode, TailwindVersion, TokenMode};
        let node = self.doc().node_or_master(&node_id(&a.id));
        let node = node.ok_or_else(|| ToolError::not_found(&a.id))?;
        let format = match a.format {
            CodeFormatArg::Html => CodeFormat::Html,
            CodeFormatArg::Tailwind => CodeFormat::Tailwind,
            CodeFormatArg::React => CodeFormat::React,
        };
        let o = a.options;
        let file = self
            .snap
            .path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let opts = CodeOptions {
            file: &file,
            revision: self.snap.revision,
            tokens: match o.tokens {
                Some(TokenModeArg::Inline) => TokenMode::Inline,
                _ => TokenMode::CssVars,
            },
            include_components: o.include_components.unwrap_or(true),
            responsive: o.responsive.unwrap_or(false),
            tailwind: match o.tailwind {
                Some(TailwindArg::V3) => TailwindVersion::V3,
                _ => TailwindVersion::V4,
            },
            images: ImageMode::Files,
            assets: Some(&self.snap.assets),
            node_ids: false,
        };
        let files = export::export_code(self.doc(), &self.snap.geometry, &node.id, format, &opts)
            .map_err(|e| ToolError::internal(e.to_string()))?;
        let files: Vec<Value> = files
            .into_iter()
            .map(|f| match f.content {
                FileContent::Text(t) => {
                    json!({ "path": f.path, "language": f.language, "content": t })
                }
                FileContent::Binary(b) => json!({
                    "path": f.path,
                    "language": f.language,
                    "encoding": "base64",
                    "content": base64::engine::general_purpose::STANDARD.encode(b),
                }),
            })
            .collect();
        ok(json!({ "revision": self.snap.revision, "files": files }))
    }

    async fn set_tokens(&self, a: SetTokensArgs) -> Result<ToolOutput, ToolError> {
        fn map<V, W>(
            m: std::collections::BTreeMap<String, Option<V>>,
            f: impl Fn(V) -> W,
        ) -> std::collections::BTreeMap<TokenId, Option<W>> {
            m.into_iter()
                .map(|(k, v)| (TokenId::new(k), v.map(&f)))
                .collect()
        }
        for (k, v) in &a.colors {
            if let Some(c) = v {
                if !Color::new(c.clone()).is_well_formed() {
                    return Err(ToolError::invalid_args(format!(
                        "colors.{k}: {c:?} is not a #rrggbb[aa] color"
                    )));
                }
            }
        }
        let patch = TokensPatch {
            colors: map(a.colors, Color::new),
            spacing: map(a.spacing, |v| v),
            radii: map(a.radii, |v| v),
            fonts: map(a.fonts, |v| v),
        };
        if patch == TokensPatch::default() {
            return Err(ToolError::invalid_args("no tokens given"));
        }
        let commands = vec![Command::UpsertTokens { patch }];
        let tx = self
            .commit(commands, "Agent: set tokens".into(), a.if_revision, None)
            .await?;
        ok(json!({ "revision": tx.revision }))
    }

    async fn upsert_style(&self, a: UpsertStyleArgs) -> Result<ToolOutput, ToolError> {
        let id = match &a.id {
            Some(id) => {
                let id = node_id(id);
                if !self.doc().styles.iter().any(|s| s.id == id) {
                    return Err(ToolError::not_found(id));
                }
                id
            }
            None => Id::new(IdKind::Style),
        };
        let mut v = a.value;
        for key in ["id", "name", "kind"] {
            if v.contains_key(key) {
                return Err(ToolError::invalid_args(format!(
                    "value: \"{key}\" is a separate argument"
                )));
            }
        }
        v.insert("id".into(), json!(id));
        v.insert("name".into(), json!(a.name));
        v.insert("kind".into(), json!(a.kind.tag()));
        let style: Style = serde_json::from_value(Value::Object(v))
            .map_err(|e| ToolError::invalid_args(format!("value: {e}")))?;
        let verb = if a.id.is_some() { "update" } else { "create" };
        let label = format!("Agent: {verb} style {}", style.name);
        let tx = self
            .commit(
                vec![Command::UpsertStyle { style }],
                label,
                a.if_revision,
                None,
            )
            .await?;
        ok(json!({ "revision": tx.revision, "id": id }))
    }

    async fn apply_style(&self, a: ApplyStyleArgs) -> Result<ToolOutput, ToolError> {
        if a.node_ids.is_empty() {
            return Err(ToolError::invalid_args("node_ids is empty"));
        }
        let nodes = a
            .node_ids
            .iter()
            .map(|id| self.node(id))
            .collect::<Result<Vec<_>, _>>()?;
        let ids: Vec<NodeId> = nodes.iter().map(|n| n.id.clone()).collect();
        let mut commands = Vec::new();
        let label = match &a.style_id {
            Some(style_id) => {
                let style = self
                    .doc()
                    .styles
                    .iter()
                    .find(|s| s.id.as_str() == style_id)
                    .ok_or_else(|| ToolError::not_found(style_id))?;
                let slot = a.slot.unwrap_or(style_kind_arg(&style.kind));
                for n in &nodes {
                    let mut patch = Map::new();
                    patch.insert(slot_field(slot).into(), json!(style.id));
                    commands.push(self.patch_command(n.id.as_str(), patch)?);
                }
                format!("Agent: apply style {}", style.name)
            }
            None => {
                for n in &nodes {
                    let only = match a.slot {
                        Some(slot) => match slot_ref(n, slot) {
                            Some(id) => Some(id),
                            None => continue,
                        },
                        None => None,
                    };
                    if let Some(patch) = detach_style_patch(self.doc(), n, only.as_ref()) {
                        commands.push(Command::SetProps {
                            id: n.id.clone(),
                            patch,
                        });
                    }
                }
                layers_label("detach style from", self.doc(), &ids)
            }
        };
        if commands.is_empty() {
            return ok(json!({ "revision": self.snap.revision, "ids": ids }));
        }
        let tx = self.commit(commands, label, a.if_revision, None).await?;
        ok(json!({ "revision": tx.revision, "ids": ids }))
    }

    async fn set_selection(&self, a: SetSelectionArgs) -> Result<ToolOutput, ToolError> {
        let ids = a
            .ids
            .iter()
            .map(|id| self.node(id).map(|n| n.id.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let ids = self.app.set_selection(ids, a.zoom_to).await?;
        ok(json!({ "ids": ids }))
    }

    // -- Look and export ----------------------------------------------------

    async fn screenshot(&self, a: ScreenshotArgs) -> Result<ToolOutput, ToolError> {
        let node = self.node(&a.id)?;
        check_scale(a.scale)?;
        if a.max_size == 0 {
            return Err(ToolError::invalid_args("max_size must be positive"));
        }
        let background = match a.background {
            Some(c) => {
                let c = Color::new(c);
                if !c.is_well_formed() {
                    return Err(ToolError::invalid_args(
                        "background is not a #rrggbb[aa] color",
                    ));
                }
                Some(c)
            }
            None => None,
        };
        let shot = self
            .app
            .screenshot(ScreenshotRequest {
                node: node.id.clone(),
                scale: a.scale,
                max_size: a.max_size,
                background,
            })
            .await?;
        Ok(ToolOutput::Image {
            info: json!({
                "width": shot.width,
                "height": shot.height,
                "scale": round2(shot.scale as f64),
            }),
            png: shot.png,
        })
    }

    async fn export_file(&self, a: ExportFileArgs) -> Result<ToolOutput, ToolError> {
        let node = self.node(&a.id)?;
        check_scale(a.scale)?;
        let doc_path = self.snap.path.as_deref();
        let mut path = PathBuf::from(&a.path);
        if path.is_relative() {
            let Some(dir) = doc_path.and_then(|p| p.parent()) else {
                return Err(ToolError::invalid_args(
                    "a relative path needs a saved document; give an absolute path",
                ));
            };
            path = dir.join(path);
        }
        let format = match a.format {
            ExportFormatArg::Png => ExportFormat::Png,
            ExportFormatArg::Svg => ExportFormat::Svg,
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = check_export_path(&path, format, doc_path, home.as_deref())?;
        let bytes = self
            .app
            .export_file(ExportRequest {
                node: node.id.clone(),
                format,
                path: path.clone(),
                scale: a.scale,
            })
            .await?;
        ok(json!({ "path": path, "bytes": bytes }))
    }
}

/// `get_document`'s result (also `design://document`).
pub fn document_json(
    snap: &Snapshot,
    page_id: Option<&str>,
    depth: u32,
) -> Result<Value, ToolError> {
    let doc = &snap.document;
    let geometry = &snap.geometry;
    let pages: Vec<_> = match page_id {
        Some(id) => vec![doc
            .page(&node_id(id))
            .ok_or_else(|| ToolError::not_found(id))?],
        None => doc.pages.iter().collect(),
    };
    let pages: Vec<Value> = pages
        .into_iter()
        .map(|p| {
            let children =
                children_value(&p.children, depth, |n| summary_tree(n, geometry, depth - 1));
            json!({ "id": p.id, "name": p.name, "children": children })
        })
        .collect();
    let styles: Vec<Value> = doc
        .styles
        .iter()
        .map(|s| json!({ "id": s.id, "name": s.name, "kind": style_kind(&s.kind) }))
        .collect();
    let components: Vec<Value> = doc
        .components
        .iter()
        .map(|c| json!({ "id": c.id, "name": c.name }))
        .collect();
    Ok(json!({
        "revision": snap.revision,
        "title": doc.meta.title,
        "path": snap.path,
        "dirty": snap.dirty,
        "pages": pages,
        "tokens": file_json(&doc.tokens),
        "styles": styles,
        "components": components,
    }))
}

fn check_scale(scale: f32) -> Result<(), ToolError> {
    if scale.is_finite() && scale > 0.0 && scale <= 16.0 {
        Ok(())
    } else {
        Err(ToolError::invalid_args("scale must be in (0, 16]"))
    }
}

fn parent_id(p: &ParentRef) -> &Id {
    match p {
        ParentRef::Page(id) | ParentRef::Node(id) => id,
    }
}

/// Page name, then each ancestor's name, then the node's.
fn name_path(doc: &Document, id: &NodeId) -> Vec<String> {
    let mut names = Vec::new();
    let mut cur = id.clone();
    loop {
        names.push(doc.node(&cur).map(|n| n.name.clone()).unwrap_or_default());
        match doc.parent_of(&cur) {
            Some((ParentRef::Node(p), _)) => cur = p,
            Some((ParentRef::Page(p), _)) => {
                names.push(doc.page(&p).map(|p| p.name.clone()).unwrap_or_default());
                break;
            }
            None => break,
        }
    }
    names.reverse();
    names
}

/// The page-level node containing `id`.
fn top_level(doc: &Document, id: &NodeId) -> Option<NodeId> {
    let mut cur = id.clone();
    loop {
        match doc.parent_of(&cur)? {
            (ParentRef::Node(p), _) => cur = p,
            (ParentRef::Page(_), _) => return Some(cur),
        }
    }
}

/// `{ page_id, ids, nodes, focused_frame_id? }` (also `design://selection`).
pub fn selection_json(snap: &Snapshot) -> Value {
    let doc = &snap.document;
    let ids: Vec<&NodeId> = snap
        .selection
        .ids
        .iter()
        .filter(|id| doc.node(id).is_some())
        .collect();
    let nodes: Vec<Value> = ids
        .iter()
        .filter_map(|id| doc.node(id))
        .map(|n| summary(n, &snap.geometry))
        .collect();
    let mut out = Map::new();
    out.insert("page_id".into(), json!(snap.selection.page_id));
    out.insert("ids".into(), json!(ids));
    out.insert("nodes".into(), json!(nodes));
    let focused: BTreeSet<NodeId> = ids
        .iter()
        .filter_map(|id| top_level(doc, id))
        .filter(|id| matches!(doc.node(id).map(|n| &n.kind), Some(NodeKind::Frame(_))))
        .collect();
    if focused.len() == 1 {
        out.insert("focused_frame_id".into(), json!(focused.first()));
    }
    Value::Object(out)
}

fn style_kind(k: &StyleKind) -> &'static str {
    style_kind_arg(k).tag()
}

fn style_kind_arg(k: &StyleKind) -> StyleKindArg {
    match k {
        StyleKind::Fill(_) => StyleKindArg::Fill,
        StyleKind::Stroke(_) => StyleKindArg::Stroke,
        StyleKind::Text(_) => StyleKindArg::Text,
        StyleKind::Effect(_) => StyleKindArg::Effect,
    }
}

fn slot_field(slot: StyleKindArg) -> &'static str {
    match slot {
        StyleKindArg::Fill => "fill_style",
        StyleKindArg::Stroke => "stroke_style",
        StyleKindArg::Text => "text_style",
        StyleKindArg::Effect => "effect_style",
    }
}

/// The style a node references in `slot`.
fn slot_ref(n: &Node, slot: StyleKindArg) -> Option<doc::StyleId> {
    match (&n.kind, slot) {
        (NodeKind::Text(t), StyleKindArg::Text) => t.text_style.clone(),
        (_, StyleKindArg::Fill) => n.shape()?.fill_style.clone(),
        (_, StyleKindArg::Stroke) => n.shape()?.stroke_style.clone(),
        (_, StyleKindArg::Effect) => n.shape()?.effect_style.clone(),
        _ => None,
    }
}
