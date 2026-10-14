//! The Export window (§8): pick a format for the selected node, see the
//! files it makes, copy one to the clipboard or save them all to a folder.
//! Code and SVG are made from the document; PNG is rendered by the canvas
//! when saved, so the preview only lists its file name.

use std::path::{Path, PathBuf};

use doc::NodeId;
use export::{
    export_code, export_svg, CodeFormat, CodeOptions, FileContent, OutFile, SvgOptions,
    TailwindVersion, TokenFormat, TokenMode,
};

use crate::editor::Editor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// PNG at 1x, 2x or 3x.
    Png(u32),
    Svg,
    Html,
    Tailwind(TailwindVersion),
    React,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Png(1),
        Kind::Png(2),
        Kind::Png(3),
        Kind::Svg,
        Kind::Html,
        Kind::Tailwind(TailwindVersion::V4),
        Kind::Tailwind(TailwindVersion::V3),
        Kind::React,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Png(1) => "PNG 1x",
            Kind::Png(2) => "PNG 2x",
            Kind::Png(_) => "PNG 3x",
            Kind::Svg => "SVG",
            Kind::Html => "HTML + CSS",
            Kind::Tailwind(TailwindVersion::V4) => "Tailwind v4",
            Kind::Tailwind(TailwindVersion::V3) => "Tailwind v3",
            Kind::React => "React (TSX + Tailwind)",
        }
    }

    /// Whether the token mode changes the output.
    fn uses_tokens(self) -> bool {
        !matches!(self, Kind::Png(_) | Kind::Svg)
    }
}

/// What the window asks the app to do after a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Put this text on the clipboard.
    Copy(String),
    /// Ask for a folder and write every file there.
    Save,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    node: NodeId,
    revision: u64,
    kind: Kind,
    tokens: TokenMode,
}

pub struct ExportWindow {
    pub open: bool,
    pub kind: Kind,
    pub tokens: TokenMode,
    /// The file shown in the preview.
    pub selected: usize,
    cache: Option<(Key, Result<Vec<OutFile>, String>)>,
}

impl Default for ExportWindow {
    fn default() -> Self {
        ExportWindow {
            open: false,
            kind: Kind::Html,
            tokens: TokenMode::CssVars,
            selected: 0,
            cache: None,
        }
    }
}

/// The node the window exports: the first selected one.
pub fn target(ed: &Editor) -> Option<NodeId> {
    ed.selection.first().cloned()
}

/// A file-name stem from a layer name.
fn stem(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "export".into()
    } else {
        s
    }
}

/// The files `kind` makes for `id`. PNG files are listed with empty content;
/// [`ExportWindow::needs_render`] tells the caller to render them.
pub fn plan(
    ed: &Editor,
    id: &NodeId,
    kind: Kind,
    tokens: TokenMode,
) -> Result<Vec<OutFile>, String> {
    let node = ed
        .document()
        .node_or_master(id)
        .ok_or_else(|| format!("{id} is not in the document"))?;
    let file = ed.display_name();
    let stem = stem(&node.name);
    let err = |e: export::ExportError| e.to_string();
    match kind {
        Kind::Png(scale) => {
            let path = if scale == 1 {
                format!("{stem}.png")
            } else {
                format!("{stem}@{scale}x.png")
            };
            Ok(vec![OutFile {
                path,
                language: "png".into(),
                content: FileContent::Binary(Vec::new()),
            }])
        }
        Kind::Svg => {
            let opts = SvgOptions {
                file: &file,
                revision: ed.revision,
                assets: Some(&ed.assets),
            };
            let svg = export_svg(ed.document(), ed.document_geometry(), id, &opts).map_err(err)?;
            Ok(vec![OutFile {
                path: format!("{stem}.svg"),
                language: "svg".into(),
                content: FileContent::Text(svg),
            }])
        }
        Kind::Html | Kind::Tailwind(_) | Kind::React => {
            let (format, tailwind) = match kind {
                Kind::Html => (CodeFormat::Html, TailwindVersion::V4),
                Kind::Tailwind(v) => (CodeFormat::Tailwind, v),
                _ => (CodeFormat::React, TailwindVersion::V4),
            };
            let opts = CodeOptions {
                file: &file,
                revision: ed.revision,
                tokens,
                tailwind,
                assets: Some(&ed.assets),
                ..Default::default()
            };
            export_code(ed.document(), ed.document_geometry(), id, format, &opts).map_err(err)
        }
    }
}

/// The document's tokens as one file.
pub fn tokens_file(ed: &Editor, format: TokenFormat) -> OutFile {
    export::export_tokens(ed.document(), format)
}

/// Write `files` under `dir`, making folders as needed. Returns the paths
/// written.
pub fn write_files(files: &[OutFile], dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for f in files {
        let path = dir.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match &f.content {
            FileContent::Text(t) => std::fs::write(&path, t)?,
            FileContent::Binary(b) => std::fs::write(&path, b)?,
        }
        written.push(path);
    }
    Ok(written)
}

impl ExportWindow {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Whether the current format is rendered by the canvas on save.
    pub fn needs_render(&self) -> Option<u32> {
        match self.kind {
            Kind::Png(s) => Some(s),
            _ => None,
        }
    }

    /// The files for the current choices, cached until they or the document
    /// change.
    pub fn files(&mut self, ed: &Editor) -> Result<&[OutFile], String> {
        let id = target(ed).ok_or("Select a layer to export")?;
        let key = Key {
            node: id.clone(),
            revision: ed.revision,
            kind: self.kind,
            tokens: self.tokens,
        };
        if self.cache.as_ref().map(|(k, _)| k) != Some(&key) {
            let files = plan(ed, &id, self.kind, self.tokens);
            self.cache = Some((key, files));
        }
        match &self.cache.as_ref().expect("just filled").1 {
            Ok(f) => Ok(f),
            Err(e) => Err(e.clone()),
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context, ed: &Editor) -> Option<Request> {
        if !self.open {
            return None;
        }
        let mut request = None;
        let mut open = true;
        egui::Window::new("Export")
            .open(&mut open)
            .default_width(460.0)
            .show(ctx, |ui| request = self.contents(ui, ed));
        if !open {
            self.open = false;
        }
        request
    }

    fn contents(&mut self, ui: &mut egui::Ui, ed: &Editor) -> Option<Request> {
        let mut request = None;
        match target(ed).and_then(|id| ed.node(&id)) {
            Some(n) => ui.label(format!("Exporting “{}”", n.name)),
            None => ui.label("Select a layer to export."),
        };
        egui::Grid::new("export-options").show(ui, |ui| {
            ui.label("Format");
            egui::ComboBox::from_id_salt("export-format")
                .selected_text(self.kind.label())
                .show_ui(ui, |ui| {
                    for k in Kind::ALL {
                        ui.selectable_value(&mut self.kind, k, k.label());
                    }
                });
            ui.end_row();
            if self.kind.uses_tokens() {
                ui.label("Tokens");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tokens, TokenMode::CssVars, "CSS variables");
                    ui.selectable_value(&mut self.tokens, TokenMode::Inline, "Inline values");
                });
                ui.end_row();
            }
        });
        ui.separator();
        let files = self.files(ed).map(<[OutFile]>::to_vec);
        match &files {
            Err(e) => {
                ui.colored_label(ui.visuals().warn_fg_color, e);
            }
            Ok(files) => {
                self.selected = self.selected.min(files.len().saturating_sub(1));
                ui.label("Files");
                for (i, f) in files.iter().enumerate() {
                    ui.selectable_value(&mut self.selected, i, &f.path);
                }
                if let Some(text) = files.get(self.selected).and_then(OutFile::as_text) {
                    egui::ScrollArea::both().max_height(260.0).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut text.to_string().as_str())
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
                } else if self.needs_render().is_some() {
                    ui.weak("Rendered when saved.");
                }
            }
        }
        ui.horizontal(|ui| {
            let copyable = files
                .as_ref()
                .ok()
                .and_then(|f| f.get(self.selected))
                .and_then(OutFile::as_text);
            if ui
                .add_enabled(copyable.is_some(), egui::Button::new("Copy to clipboard"))
                .clicked()
            {
                request = copyable.map(|t| Request::Copy(t.to_string()));
            }
            if ui
                .add_enabled(files.is_ok(), egui::Button::new("Save to folder…"))
                .clicked()
            {
                request = Some(Request::Save);
            }
        });
        request
    }
}
