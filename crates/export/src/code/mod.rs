//! Code and token export (§8, §6.3 `export_code`): a node as HTML + CSS,
//! HTML + Tailwind, or React (TSX + Tailwind), and the document's tokens as
//! CSS custom properties or a Tailwind theme. Pure and deterministic, so the
//! output is golden-tested.
//!
//! Every format is written from one element tree ([`build`]): one element
//! per node, auto-layout as flexbox per §3.5, everything else absolutely
//! positioned. Layer names become class and component names, `semantic`
//! hints pick tags, and nothing invents behavior (no handlers, no state).

mod build;
mod html;
mod ir;
mod names;
mod react;
mod tailwind;

use doc::{Assets, Document, Node, NodeId};
use layout::Geometry;

use crate::ExportError;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CodeFormat {
    /// `index.html` + `styles.css`.
    #[default]
    Html,
    /// `index.html` with Tailwind utilities, plus the theme.
    Tailwind,
    /// TSX + Tailwind: a page component and one file per component.
    React,
}

/// How token references are written (§6.3 `options.tokens`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TokenMode {
    /// `var(--color-primary)` in CSS, theme utilities (`bg-primary`) in
    /// Tailwind.
    #[default]
    CssVars,
    /// The tokens' values.
    Inline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TailwindVersion {
    /// A CSS-first `@theme` block.
    #[default]
    V4,
    /// `tailwind.config.js`.
    V3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageMode {
    /// Image files under `assets/`, returned as binary output files.
    #[default]
    Files,
    /// `data:` URIs inline.
    DataUri,
}

#[derive(Clone, Copy, Debug)]
pub struct CodeOptions<'a> {
    /// The design file's name, for the header comment.
    pub file: &'a str,
    /// The document revision the export was made from.
    pub revision: u64,
    pub tokens: TokenMode,
    /// React: each component is its own file and instances call it.
    /// Otherwise (and in HTML) instances are expanded in place.
    pub include_components: bool,
    /// The root takes the full width up to its design width
    /// (`width: 100%; max-width`) and at least its design height.
    pub responsive: bool,
    pub tailwind: TailwindVersion,
    pub images: ImageMode,
    /// Where image bytes come from; without it images are referenced as
    /// `assets/<name>` but no files are written.
    pub assets: Option<&'a Assets>,
    /// Mark each element with `data-node="<node id>"`.
    pub node_ids: bool,
}

impl Default for CodeOptions<'_> {
    fn default() -> Self {
        CodeOptions {
            file: "",
            revision: 0,
            tokens: TokenMode::CssVars,
            include_components: true,
            responsive: false,
            tailwind: TailwindVersion::V4,
            images: ImageMode::Files,
            assets: None,
            node_ids: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileContent {
    Text(String),
    Binary(Vec<u8>),
}

/// One exported file; `path` is relative to the export folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutFile {
    pub path: String,
    /// `html`, `css`, `tsx`, `javascript`, or the image type (`png`, …).
    pub language: String,
    pub content: FileContent,
}

impl OutFile {
    fn text(path: impl Into<String>, language: &str, content: String) -> OutFile {
        OutFile {
            path: path.into(),
            language: language.to_string(),
            content: FileContent::Text(content),
        }
    }

    /// The content of a text file.
    pub fn as_text(&self) -> Option<&str> {
        match &self.content {
            FileContent::Text(s) => Some(s),
            FileContent::Binary(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenFormat {
    /// `tokens.css`: `:root` custom properties and a class per text style.
    Css,
    /// `theme.css`: a Tailwind v4 `@theme` block.
    TailwindV4,
    /// `tailwind.config.js` for Tailwind v3.
    TailwindV3,
}

/// The node and its subtree as code. Hidden nodes are left out.
pub fn export_code(
    doc: &Document,
    geom: &Geometry,
    id: &NodeId,
    format: CodeFormat,
    opts: &CodeOptions<'_>,
) -> Result<Vec<OutFile>, ExportError> {
    let node = doc
        .node_or_master(id)
        .ok_or_else(|| ExportError::NodeNotFound(id.to_string()))?;
    let (mut files, assets) = match format {
        CodeFormat::Html => html::export(doc, geom, node, opts),
        CodeFormat::Tailwind => tailwind::export(doc, geom, node, opts),
        CodeFormat::React => react::export(doc, geom, node, opts),
    };
    for a in assets {
        let Some(bytes) = opts.assets.and_then(|s| s.bytes(&a)) else {
            continue;
        };
        let ext = a.0.rsplit_once('.').map_or("bin", |(_, e)| e);
        files.push(OutFile {
            path: asset_path(&a),
            language: ext.to_ascii_lowercase(),
            content: FileContent::Binary(bytes.as_ref().clone()),
        });
    }
    Ok(files)
}

/// Where an exported asset lives, and how the code refers to it.
pub(crate) fn asset_path(a: &doc::AssetRef) -> String {
    format!("assets/{}", ir::asset_file(&a.0))
}

/// The document's tokens (and text styles) for use in code.
pub fn export_tokens(doc: &Document, format: TokenFormat) -> OutFile {
    match format {
        TokenFormat::Css => OutFile::text("tokens.css", "css", html::tokens_css(doc)),
        TokenFormat::TailwindV4 => OutFile::text("theme.css", "css", tailwind::theme_v4_file(doc)),
        TokenFormat::TailwindV3 => OutFile::text(
            "tailwind.config.js",
            "javascript",
            tailwind::config_v3_file(doc),
        ),
    }
}

/// The children of an instance: its component's root children with derived
/// IDs (`<instanceId>/<componentNodeId>`) and the instance's overrides
/// applied (§3.7), exactly as the canvas expands it. `None` if `node` is not
/// an instance or it cannot expand.
pub fn instance_children(doc: &Document, node: &Node) -> Option<Vec<Node>> {
    doc::expand_instance(doc, node)?.children().cloned()
}

/// An instance as the frame it stands for (`doc::expand_instance`).
pub(crate) fn expand_instance(doc: &Document, n: &Node) -> Option<Node> {
    doc::expand_instance(doc, n)
}
