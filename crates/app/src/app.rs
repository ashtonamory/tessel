//! The eframe app: window setup, menus, toolbar, shortcuts, file dialogs,
//! and the layers / canvas / inspector / status layout (§9).

use std::path::PathBuf;

use egui::{Key, KeyboardShortcut, Modifiers, ViewportCommand};

use crate::activity::ActivityPanel;
use crate::agent_host::AgentHost;
use crate::agent_socket;
use crate::autosave::{self, Autosaver};
use crate::canvas::{Canvas, Tool};
use crate::components_panel::ComponentsPanel;
use crate::editor::Editor;
use crate::export_window::{self, ExportWindow, Request};
use crate::inspector::Inspector;
use crate::instance_inspector::InstanceInspector;
use crate::layers::LayersPanel;
use crate::panels::{self, PanelMode, Panels, Side};
use crate::place;
use crate::structure::ZOrder;
use crate::styles_panel::StylesPanel;
use crate::tokens_panel::TokensPanel;
use crate::watch::{self, Decision, FileWatcher};

pub const APP_ID: &str = "tessel";

/// Open the main window. Returns an error, rather than panicking, when
/// there is no display or no usable GPU.
pub fn run(file: Option<PathBuf>) -> anyhow::Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        anyhow::bail!(
            "no display found: WAYLAND_DISPLAY and DISPLAY are both unset. \
             Tessel needs a Wayland (or X11) session"
        );
    }
    let editor = match &file {
        Some(p) if p.exists() => {
            Editor::open(p).map_err(|e| anyhow::anyhow!("cannot open {}: {e}", p.display()))?
        }
        Some(p) => {
            tracing::info!(
                "{} does not exist yet; it will be created on save",
                p.display()
            );
            let mut ed = Editor::new();
            ed.path = Some(p.clone());
            ed
        }
        None => Editor::new(),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(APP_ID)
            .with_title(title(&editor))
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_configuration(),
        ..Default::default()
    };
    eframe::run_native(
        APP_ID,
        options,
        Box::new(move |cc| {
            let mut app = DesignApp::new(editor);
            app.start_agent(&cc.egui_ctx, Some(&agent_socket::runtime_dir()));
            app.check_recovery();
            app.load_panels();
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("cannot open the window: {e}"))
}

/// The wgpu setup shared by egui and vello (§7.2): prefer Vulkan, fall
/// back to GL, and ask for the adapter's own limits. egui's default asks a
/// GL device for WebGL2 limits, which have no compute storage buffers, so
/// vello could not run on it.
pub fn wgpu_configuration() -> egui_wgpu::WgpuConfiguration {
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends =
        wgpu::Backends::from_env().unwrap_or(wgpu::Backends::VULKAN | wgpu::Backends::GL);
    setup.device_descriptor = std::sync::Arc::new(|adapter| wgpu::DeviceDescriptor {
        label: Some("Tessel device"),
        required_limits: adapter.limits(),
        ..Default::default()
    });
    egui_wgpu::WgpuConfiguration {
        wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// "Undo Move 3 layers" (§5.3).
pub fn undo_text(ed: &Editor) -> String {
    match ed.history.undo_label() {
        Some(l) => format!("Undo {l}"),
        None => "Undo".to_string(),
    }
}

pub fn redo_text(ed: &Editor) -> String {
    match ed.history.redo_label() {
        Some(l) => format!("Redo {l}"),
        None => "Redo".to_string(),
    }
}

fn title(ed: &Editor) -> String {
    let dirty = if ed.is_dirty() { "• " } else { "" };
    let ro = if ed.read_only { " [read-only]" } else { "" };
    format!("{dirty}{}{ro} — Tessel", ed.display_name())
}

/// What to do once unsaved changes are dealt with.
#[derive(Clone, Copy, Debug, PartialEq)]
enum After {
    New,
    Open,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
enum Action {
    New,
    Open,
    Save,
    SaveAs,
    Quit,
    Undo,
    Redo,
    Delete,
    SelectAll,
    Escape,
    Tool(Tool),
    InsertImage,
    ImportSvg,
    /// Show or hide the Export window.
    ToggleExport,
    ExportTokens(export::TokenFormat),
    /// Paste whatever the system clipboard holds.
    Paste,
    /// Paste text that arrived with Ctrl+V.
    PasteText(String),
    Copy,
    Cut,
    Duplicate,
    Group,
    Ungroup,
    FrameSelection,
    AutoLayout,
    ToFront,
    ToBack,
    Nudge(f32, f32),
    Forward,
    Backward,
    ZoomFit,
    Zoom100,
    ZoomSelection,
    CreateComponent,
    DetachInstance,
    GoToMaster,
    /// Show or hide one side panel.
    TogglePanel(Side),
    /// Ctrl+\: hide both side panels, or bring them back.
    ToggleAllPanels,
    /// Detach a panel into its own window, or dock it back.
    PopOut(Side),
}

/// The tabs of the left panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LeftTab {
    #[default]
    Layers,
    Tokens,
    Styles,
    Components,
}

impl LeftTab {
    pub const ALL: [LeftTab; 4] = [
        LeftTab::Layers,
        LeftTab::Components,
        LeftTab::Tokens,
        LeftTab::Styles,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LeftTab::Layers => "Layers",
            LeftTab::Tokens => "Tokens",
            LeftTab::Styles => "Styles",
            LeftTab::Components => "Components",
        }
    }
}

pub struct DesignApp {
    ed: Editor,
    canvas: Canvas,
    layers: LayersPanel,
    tokens: TokensPanel,
    styles: StylesPanel,
    components: ComponentsPanel,
    instance_inspector: InstanceInspector,
    left_tab: LeftTab,
    tab_rects: Vec<(LeftTab, egui::Rect)>,
    inspector: Inspector,
    export: ExportWindow,
    /// The last text this app put on the clipboard (Copy and Cut).
    last_copied: Option<String>,
    confirm: Option<After>,
    allow_close: bool,
    title: String,
    activity: ActivityPanel,
    autosaver: Autosaver,
    watcher: FileWatcher,
    /// A recovery file from an earlier run, waiting for Restore or Discard.
    recovery_offer: Option<RecoveryOffer>,
    /// The open file changed on disk while there are unsaved edits.
    reload_prompt: bool,
    /// The MCP server agents connect to (§6.1); `None` until started.
    agent: Option<AgentHost>,
    /// Where each side panel lives (docked, hidden, detached).
    panels: Panels,
    /// What `ui.json` last held; `None` means do not write it.
    panels_saved: Option<(PathBuf, Panels)>,
}

/// An autosaved document found at launch (§4.5).
#[derive(Clone, Debug, PartialEq)]
pub struct RecoveryOffer {
    pub file: PathBuf,
    /// The file it recovers; `None` for an untitled document.
    pub source: Option<PathBuf>,
}

impl DesignApp {
    /// Read the remembered panel layout from `ui.json`.
    fn load_panels(&mut self) {
        if let Some(path) = panels::config_path() {
            self.panels = Panels::load(&path);
            self.panels_saved = Some((path, self.panels));
        }
    }

    /// Write `ui.json` when the layout changed, but not mid-drag: a resize
    /// changes the size every frame, so wait for the button to come up.
    fn save_panels(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        if let Some((path, saved)) = &mut self.panels_saved {
            if *saved != self.panels {
                if let Err(e) = self.panels.save(path) {
                    tracing::warn!("cannot write {}: {e}", path.display());
                }
                *saved = self.panels;
            }
        }
    }

    pub fn new(ed: Editor) -> DesignApp {
        let now = ed.clock.now();
        let mut watcher = FileWatcher::new(now);
        watcher.sync(ed.path.as_deref());
        DesignApp {
            title: title(&ed),
            autosaver: Autosaver::new(autosave::default_recovery_dir(), now),
            watcher,
            activity: ActivityPanel::default(),
            recovery_offer: None,
            reload_prompt: false,
            agent: None,
            panels: Panels::default(),
            panels_saved: None,
            ed,
            canvas: Canvas::new(),
            layers: LayersPanel::default(),
            tokens: TokensPanel::default(),
            styles: StylesPanel::default(),
            components: ComponentsPanel::default(),
            instance_inspector: InstanceInspector::default(),
            left_tab: LeftTab::default(),
            tab_rects: Vec::new(),
            inspector: Inspector::default(),
            export: ExportWindow::default(),
            last_copied: None,
            confirm: None,
            allow_close: false,
        }
    }

    pub fn editor(&self) -> &Editor {
        &self.ed
    }

    /// Mutable access for tests that set up state the UI would.
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.ed
    }

    /// Start the MCP server, listening in `socket_dir` if given.
    pub fn start_agent(&mut self, ctx: &egui::Context, socket_dir: Option<&std::path::Path>) {
        match AgentHost::start(ctx, &self.ed, socket_dir) {
            Ok(host) => self.agent = Some(host),
            Err(e) => {
                tracing::error!("cannot start the agent server: {e}");
                self.ed.status = Some(format!("Agents are off: {e}"));
            }
        }
    }

    pub fn agent_host(&self) -> Option<&AgentHost> {
        self.agent.as_ref()
    }

    /// Stop the MCP server: sessions close and the socket is removed.
    pub fn stop_agent(&mut self) {
        self.agent = None;
    }

    /// The status bar's agent indicator: "2 agents: claude-code, cursor".
    pub fn agent_indicator(&self) -> Option<String> {
        let host = self.agent.as_ref()?;
        let names: Vec<String> = host.sessions().into_iter().map(|s| s.client).collect();
        let mut text = match names.len() {
            0 => "No agents".to_string(),
            1 => format!("1 agent: {}", names[0]),
            n => format!("{n} agents: {}", names.join(", ")),
        };
        if !host.agents_may_edit() {
            text.push_str(" (read-only)");
        }
        Some(text)
    }

    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }

    pub fn layers(&self) -> &LayersPanel {
        &self.layers
    }

    pub fn inspector(&self) -> &Inspector {
        &self.inspector
    }

    pub fn tokens(&self) -> &TokensPanel {
        &self.tokens
    }

    pub fn styles(&self) -> &StylesPanel {
        &self.styles
    }

    pub fn components(&self) -> &ComponentsPanel {
        &self.components
    }

    /// Answer the "delete a used component" question.
    pub fn answer_delete_component(&mut self, delete: bool) {
        self.components.answer_delete(&mut self.ed, delete);
    }

    pub fn instance_inspector(&self) -> &InstanceInspector {
        &self.instance_inspector
    }

    pub fn left_tab(&self) -> LeftTab {
        self.left_tab
    }

    /// Where the left panel's tab button was drawn last frame.
    pub fn left_tab_rect(&self, tab: LeftTab) -> Option<egui::Rect> {
        self.tab_rects
            .iter()
            .find(|(t, _)| *t == tab)
            .map(|(_, r)| *r)
    }

    pub fn activity(&self) -> &ActivityPanel {
        &self.activity
    }

    pub fn activity_mut(&mut self) -> &mut ActivityPanel {
        &mut self.activity
    }

    pub fn recovery_offer(&self) -> Option<&RecoveryOffer> {
        self.recovery_offer.as_ref()
    }

    pub fn is_asking_to_reload(&self) -> bool {
        self.reload_prompt
    }

    /// Where autosaves go (`None` turns autosave off).
    pub fn set_recovery_dir(&mut self, dir: Option<PathBuf>) {
        self.autosaver.dir = dir;
    }

    /// Offer to restore an autosave of the open document, if an earlier
    /// run left a newer one behind (§4.5). Call once at launch.
    pub fn check_recovery(&mut self) {
        let Some(dir) = self.autosaver.dir.clone() else {
            return;
        };
        let source = self.ed.path.clone();
        self.recovery_offer = autosave::find_recovery(&dir, source.as_deref())
            .map(|file| RecoveryOffer { file, source });
    }

    /// Delete the recovery file of the open document (it is saved or gone).
    fn discard_recovery(&self) {
        // An unanswered offer is still wanted: closing the window is not a
        // decision to throw it away.
        if self.recovery_offer.is_some() {
            return;
        }
        if let Some(dir) = &self.autosaver.dir {
            autosave::remove_recovery(dir, self.ed.path.as_deref());
        }
    }

    /// The text of the last Copy or Cut.
    pub fn last_copied(&self) -> Option<&str> {
        self.last_copied.as_deref()
    }

    /// The tab bar, then the tab's panel.
    fn left_panel(&mut self, ui: &mut egui::Ui) {
        self.tab_rects.clear();
        ui.horizontal(|ui| {
            for tab in LeftTab::ALL {
                let resp = ui.selectable_label(self.left_tab == tab, tab.label());
                self.tab_rects.push((tab, resp.rect));
                if resp.clicked() {
                    self.left_tab = tab;
                }
            }
        });
        ui.separator();
        match self.left_tab {
            LeftTab::Layers => self.layers.ui(ui, &mut self.ed),
            LeftTab::Tokens => self.tokens.ui(ui, &mut self.ed),
            LeftTab::Styles => self.styles.ui(ui, &mut self.ed),
            LeftTab::Components => {
                let view = self.canvas.visible_page_rect();
                self.components.ui(ui, &mut self.ed, view)
            }
        }
    }

    fn replace_editor(&mut self, mut ed: Editor) {
        ed.clock = self.ed.clock.clone();
        self.ed = ed;
        self.watcher.sync(self.ed.path.as_deref());
        self.reload_prompt = false;
        self.canvas.reset();
        self.layers = LayersPanel::default();
        self.tokens = TokensPanel::default();
        self.styles = StylesPanel::default();
        self.components = ComponentsPanel::default();
        self.instance_inspector = InstanceInspector::default();
        self.inspector = Inspector::default();
        if let Some(host) = &mut self.agent {
            host.document_replaced();
        }
    }

    fn save(&mut self, save_as: bool) -> bool {
        self.canvas.interrupt(&mut self.ed);
        let path = match (&self.ed.path, save_as) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new()
                .set_title("Save design")
                .add_filter("Design", &["tessel.json", "json"])
                .set_file_name(format!("{}.tessel.json", self.ed.doc.meta.title))
                .save_file(),
        };
        let Some(path) = path else {
            return false;
        };
        let previous = self.ed.path.clone();
        match self.ed.save_as(&path) {
            Ok(()) => {
                // Our own write must not look like an external change.
                self.watcher.sync(Some(&path));
                self.reload_prompt = false;
                if let Some(dir) = &self.autosaver.dir {
                    autosave::remove_recovery(dir, previous.as_deref());
                    autosave::remove_recovery(dir, Some(&path));
                }
                self.ed.status = Some(format!("Saved {}", path.display()));
                true
            }
            Err(e) => {
                tracing::error!("save {}: {e:#}", path.display());
                self.ed.status = Some(format!("Save failed: {e}"));
                false
            }
        }
    }

    fn open_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Open design")
            .add_filter("Design", &["tessel.json", "design.json", "json"])
            .pick_file()
        else {
            return;
        };
        match Editor::open(&path) {
            Ok(ed) => {
                let problems = ed.problems.len();
                self.discard_recovery();
                self.replace_editor(ed);
                if problems > 0 {
                    self.ed.status = Some(format!(
                        "Opened read-only: {problems} problem(s) found in the file"
                    ));
                }
            }
            Err(e) => {
                tracing::error!("open {}: {e:#}", path.display());
                self.ed.status = Some(format!("Cannot open {}: {e}", path.display()));
            }
        }
    }

    /// Run `after` now, or ask about unsaved changes first.
    fn guarded(&mut self, ctx: &egui::Context, after: After) {
        if self.ed.is_dirty() {
            self.confirm = Some(after);
        } else {
            self.proceed(ctx, after);
        }
    }

    fn proceed(&mut self, ctx: &egui::Context, after: After) {
        match after {
            After::New => {
                self.discard_recovery();
                self.replace_editor(Editor::new());
            }
            After::Open => self.open_dialog(),
            After::Quit => {
                self.discard_recovery();
                self.allow_close = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    /// Place a picked or pasted image in the middle of the visible canvas.
    fn insert_image(&mut self, picked: Option<std::io::Result<place::Picked>>) {
        let view = self.canvas.visible_page_rect();
        let result = match picked {
            None => return,
            Some(Err(e)) => Err(format!("Cannot read the image: {e}")),
            Some(Ok(p)) => self.ed.insert_image(p.bytes, &p.ext, &p.name, view),
        };
        if let Err(e) = result {
            self.ed.status = Some(e);
        }
    }

    /// Import the shapes of SVG text in the middle of the visible canvas.
    pub fn import_svg_text(&mut self, svg: &str) {
        let view = self.canvas.visible_page_rect();
        if let Err(e) = self.ed.import_svg(svg, view) {
            self.ed.status = Some(e);
        }
    }

    fn import_svg_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import SVG")
            .add_filter("SVG", &["svg"])
            .pick_file()
        else {
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => self.import_svg_text(&text),
            Err(e) => self.ed.status = Some(format!("Cannot read {}: {e}", path.display())),
        }
    }

    /// Export the document's tokens to a file the user picks.
    fn export_tokens_dialog(&mut self, format: export::TokenFormat) {
        let file = export_window::tokens_file(&self.ed, format);
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export tokens")
            .set_file_name(&file.path)
            .save_file()
        else {
            return;
        };
        self.export_tokens_to(format, &path);
    }

    /// Write the document's tokens to `path`.
    pub fn export_tokens_to(&mut self, format: export::TokenFormat, path: &std::path::Path) {
        let file = export_window::tokens_file(&self.ed, format);
        let dir = path.parent().unwrap_or(std::path::Path::new("."));
        let named = export::OutFile {
            path: path
                .file_name()
                .map_or(file.path.clone(), |n| n.to_string_lossy().into_owned()),
            ..file
        };
        self.ed.status = Some(match export_window::write_files(&[named], dir) {
            Ok(_) => format!("Exported tokens to {}", path.display()),
            Err(e) => format!("Cannot write {}: {e}", path.display()),
        });
    }

    pub fn export_window(&self) -> &ExportWindow {
        &self.export
    }

    /// The files the Export window currently lists.
    pub fn export_files(&mut self) -> Result<Vec<export::OutFile>, String> {
        self.export.files(&self.ed).map(<[_]>::to_vec)
    }

    pub fn export_window_mut(&mut self) -> &mut ExportWindow {
        &mut self.export
    }

    /// Write every file of the Export window's current choices into `dir`:
    /// code, SVG or the rendered PNG, plus the image assets.
    pub fn export_save_to(&mut self, frame: &eframe::Frame, dir: &std::path::Path) {
        let status = match self.export_files_for_save(frame) {
            Ok(files) => match export_window::write_files(&files, dir) {
                Ok(paths) => format!("Exported {} file(s) to {}", paths.len(), dir.display()),
                Err(e) => format!("Cannot write to {}: {e}", dir.display()),
            },
            Err(e) => format!("Export failed: {e}"),
        };
        self.ed.status = Some(status);
    }

    fn export_files_for_save(
        &mut self,
        frame: &eframe::Frame,
    ) -> Result<Vec<export::OutFile>, String> {
        let mut files = self.export.files(&self.ed)?.to_vec();
        if let (Some(scale), Some(id)) =
            (self.export.needs_render(), export_window::target(&self.ed))
        {
            let opts = export::PngOptions {
                scale: scale as f64,
                background: None,
            };
            let png = self.canvas.render_png(frame, &self.ed, &id, &opts)?;
            files[0].content = export::FileContent::Binary(png);
        }
        Ok(files)
    }

    fn export_save_dialog(&mut self, frame: &eframe::Frame) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Export to folder")
            .pick_folder()
        else {
            return;
        };
        self.export_save_to(frame, &dir);
    }

    /// Paste copied nodes, or else an image from the clipboard.
    fn paste(&mut self, text: &str) {
        if self.ed.paste_text(text) {
            return;
        }
        match place::clipboard_image() {
            Some(p) => self.insert_image(Some(Ok(p))),
            None => self.ed.status = Some("Nothing to paste".into()),
        }
    }

    fn run_action(&mut self, ctx: &egui::Context, a: Action) {
        if matches!(
            a,
            Action::New
                | Action::Open
                | Action::Save
                | Action::SaveAs
                | Action::Undo
                | Action::Redo
        ) {
            self.canvas.interrupt(&mut self.ed);
        }
        let ed = &mut self.ed;
        match a {
            Action::New => self.guarded(ctx, After::New),
            Action::Open => self.guarded(ctx, After::Open),
            Action::Save => {
                self.save(false);
            }
            Action::SaveAs => {
                self.save(true);
            }
            Action::Quit => self.guarded(ctx, After::Quit),
            Action::TogglePanel(side) => self.panels.get_mut(side).toggle(),
            Action::ToggleAllPanels => self.panels.toggle_all(),
            Action::PopOut(side) => {
                let p = self.panels.get_mut(side);
                p.set(if p.mode == PanelMode::Detached {
                    PanelMode::Docked
                } else {
                    PanelMode::Detached
                });
            }
            Action::Undo => ed.undo(),
            Action::Redo => ed.redo(),
            Action::Delete => ed.delete_selection(),
            Action::SelectAll => {
                ed.select_all();
            }
            // Esc with nothing selected leaves a component's master.
            Action::Escape if ed.selection.is_empty() && !self.canvas.is_busy() => {
                ed.leave_component()
            }
            Action::Escape => self.canvas.cancel(ed),
            Action::Tool(t) => self.canvas.tool = t,
            Action::InsertImage => self.insert_image(place::pick_file()),
            Action::ImportSvg => self.import_svg_dialog(),
            Action::ToggleExport => self.export.toggle(),
            Action::ExportTokens(f) => self.export_tokens_dialog(f),
            Action::Paste => {
                let text = arboard::Clipboard::new()
                    .and_then(|mut c| c.get_text())
                    .ok();
                self.paste(text.as_deref().unwrap_or(""));
            }
            Action::PasteText(text) => self.paste(&text),
            Action::Copy | Action::Cut => {
                let text = if a == Action::Copy {
                    ed.copy_text()
                } else {
                    ed.cut_text()
                };
                if let Some(text) = text {
                    ctx.copy_text(text.clone());
                    self.last_copied = Some(text);
                }
            }
            Action::Duplicate => ed.duplicate_selection(),
            Action::Group => ed.group_selection(),
            Action::Ungroup => ed.ungroup_selection(),
            Action::FrameSelection => ed.frame_selection(),
            Action::AutoLayout => ed.toggle_auto_layout(),
            Action::ToFront => ed.reorder(ZOrder::Front),
            Action::ToBack => ed.reorder(ZOrder::Back),
            Action::Nudge(dx, dy) => ed.nudge(dx, dy),
            Action::Forward => ed.shift_selection(1),
            Action::Backward => ed.shift_selection(-1),
            Action::ZoomFit => self.canvas.zoom_to_fit(ed, false),
            Action::Zoom100 => self.canvas.zoom_to(1.0),
            Action::ZoomSelection => self.canvas.zoom_to_fit(ed, true),
            Action::CreateComponent => ed.create_component_from_selection(),
            Action::DetachInstance => ed.detach_selection(),
            Action::GoToMaster => {
                let target = ed.selection.first().cloned();
                if !target.is_some_and(|id| ed.go_to_master(&id)) {
                    ed.status = Some("Select an instance to go to its master".into());
                }
            }
        }
    }

    fn shortcuts(&self, ctx: &egui::Context) -> Vec<Action> {
        let typing = ctx.egui_wants_keyboard_input()
            || self.layers.is_renaming()
            || self.tokens.is_editing()
            || self.styles.is_editing()
            || self.components.is_renaming()
            || self.inspector.is_editing();
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let cmd_alt = Modifiers::COMMAND | Modifiers::ALT;
        let mut out = Vec::new();
        ctx.input_mut(|i| {
            let mut sc = |m: Modifiers, k: Key, a: Action| {
                if i.consume_shortcut(&KeyboardShortcut::new(m, k)) {
                    out.push(a);
                }
            };
            // Shift variants first: consume_shortcut ignores extra Shift.
            sc(cmd_shift, Key::K, Action::InsertImage);
            sc(cmd_shift, Key::E, Action::ToggleExport);
            sc(cmd_shift, Key::S, Action::SaveAs);
            sc(cmd, Key::S, Action::Save);
            sc(cmd, Key::O, Action::Open);
            sc(cmd, Key::N, Action::New);
            sc(cmd, Key::Q, Action::Quit);
            sc(cmd, Key::Backslash, Action::ToggleAllPanels);
            sc(cmd_shift, Key::L, Action::TogglePanel(Side::Left));
            sc(cmd_shift, Key::I, Action::TogglePanel(Side::Right));
            if !typing {
                sc(cmd_shift, Key::Z, Action::Redo);
                sc(cmd, Key::Z, Action::Undo);
                sc(cmd, Key::Y, Action::Redo);
                sc(cmd, Key::A, Action::SelectAll);
                sc(cmd_alt, Key::G, Action::FrameSelection);
                sc(cmd_alt, Key::K, Action::CreateComponent);
                sc(cmd_alt, Key::B, Action::DetachInstance);
                sc(cmd_shift, Key::G, Action::Ungroup);
                sc(cmd, Key::G, Action::Group);
                sc(cmd, Key::D, Action::Duplicate);
                sc(cmd_shift, Key::CloseBracket, Action::ToFront);
                sc(cmd_shift, Key::OpenBracket, Action::ToBack);
                sc(cmd, Key::CloseBracket, Action::Forward);
                sc(cmd, Key::OpenBracket, Action::Backward);
                sc(Modifiers::SHIFT, Key::A, Action::AutoLayout);
                sc(Modifiers::SHIFT, Key::Num1, Action::ZoomFit);
                sc(Modifiers::SHIFT, Key::Num0, Action::Zoom100);
                sc(Modifiers::SHIFT, Key::Num2, Action::ZoomSelection);
                for (k, dx, dy) in [
                    (Key::ArrowLeft, -1.0, 0.0),
                    (Key::ArrowRight, 1.0, 0.0),
                    (Key::ArrowUp, 0.0, -1.0),
                    (Key::ArrowDown, 0.0, 1.0),
                ] {
                    sc(Modifiers::SHIFT, k, Action::Nudge(dx * 10.0, dy * 10.0));
                    sc(Modifiers::NONE, k, Action::Nudge(dx, dy));
                }
                sc(Modifiers::NONE, Key::Delete, Action::Delete);
                sc(Modifiers::NONE, Key::Backspace, Action::Delete);
                sc(Modifiers::NONE, Key::Escape, Action::Escape);
                for t in Tool::ALL {
                    sc(Modifiers::NONE, t.key(), Action::Tool(t));
                }
            }
        });
        // egui turns Ctrl+V into a paste event instead of a key press.
        if !typing {
            ctx.input(|i| {
                for e in &i.events {
                    match e {
                        egui::Event::Paste(text) => out.push(Action::PasteText(text.clone())),
                        egui::Event::Copy => out.push(Action::Copy),
                        egui::Event::Cut => out.push(Action::Cut),
                        _ => {}
                    }
                }
            });
        }
        out
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let mut out = Vec::new();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                for (label, sc, a) in [
                    ("New", "Ctrl+N", Action::New),
                    ("Open…", "Ctrl+O", Action::Open),
                    ("Save", "Ctrl+S", Action::Save),
                    ("Save As…", "Ctrl+Shift+S", Action::SaveAs),
                    ("Import SVG…", "", Action::ImportSvg),
                    ("Export…", "Ctrl+Shift+E", Action::ToggleExport),
                    ("Quit", "Ctrl+Q", Action::Quit),
                ] {
                    if ui.add(egui::Button::new(label).shortcut_text(sc)).clicked() {
                        out.push(a);
                    }
                }
                ui.menu_button("Export tokens", |ui| {
                    for (label, f) in [
                        ("CSS variables", export::TokenFormat::Css),
                        ("Tailwind v4 theme", export::TokenFormat::TailwindV4),
                        ("Tailwind v3 config", export::TokenFormat::TailwindV3),
                    ] {
                        if ui.button(label).clicked() {
                            out.push(Action::ExportTokens(f));
                        }
                    }
                });
            });
            ui.menu_button("Edit", |ui| {
                let undo = undo_text(&self.ed);
                let redo = redo_text(&self.ed);
                if ui
                    .add_enabled(
                        self.ed.history.can_undo(),
                        egui::Button::new(undo).shortcut_text("Ctrl+Z"),
                    )
                    .clicked()
                {
                    out.push(Action::Undo);
                }
                if ui
                    .add_enabled(
                        self.ed.history.can_redo(),
                        egui::Button::new(redo).shortcut_text("Ctrl+Shift+Z"),
                    )
                    .clicked()
                {
                    out.push(Action::Redo);
                }
                ui.separator();
                for (label, sc, a) in [
                    ("Place image…", "Ctrl+Shift+K", Action::InsertImage),
                    ("Cut", "Ctrl+X", Action::Cut),
                    ("Copy", "Ctrl+C", Action::Copy),
                    ("Paste", "Ctrl+V", Action::Paste),
                    ("Duplicate", "Ctrl+D", Action::Duplicate),
                    ("Select all", "Ctrl+A", Action::SelectAll),
                    ("Delete", "Del", Action::Delete),
                    ("Group", "Ctrl+G", Action::Group),
                    ("Ungroup", "Ctrl+Shift+G", Action::Ungroup),
                    ("Frame selection", "Ctrl+Alt+G", Action::FrameSelection),
                    ("Bring to front", "Ctrl+Shift+]", Action::ToFront),
                    ("Bring forward", "Ctrl+]", Action::Forward),
                    ("Send backward", "Ctrl+[", Action::Backward),
                    ("Send to back", "Ctrl+Shift+[", Action::ToBack),
                ] {
                    if ui.add(egui::Button::new(label).shortcut_text(sc)).clicked() {
                        out.push(a);
                    }
                }
            });
            ui.menu_button("Components", |ui| {
                for (label, sc, a) in [
                    ("Create component", "Ctrl+Alt+K", Action::CreateComponent),
                    ("Detach instance", "Ctrl+Alt+B", Action::DetachInstance),
                    ("Go to master", "", Action::GoToMaster),
                ] {
                    if ui.add(egui::Button::new(label).shortcut_text(sc)).clicked() {
                        out.push(a);
                    }
                }
                if self.ed.editing_component().is_some() && ui.button("Leave component").clicked() {
                    self.ed.leave_component();
                }
            });
            ui.menu_button("View", |ui| {
                for (label, sc, a) in [
                    ("Zoom to fit", "Shift+1", Action::ZoomFit),
                    ("Zoom to selection", "Shift+2", Action::ZoomSelection),
                    ("Zoom to 100%", "Shift+0", Action::Zoom100),
                ] {
                    if ui.add(egui::Button::new(label).shortcut_text(sc)).clicked() {
                        out.push(a);
                    }
                }
                ui.separator();
                for (label, sc, side) in [
                    ("Left panel", "Ctrl+Shift+L", Side::Left),
                    ("Right panel", "Ctrl+Shift+I", Side::Right),
                ] {
                    let shown = self.panels.get(side).shown();
                    if ui
                        .add(egui::Button::new(label).shortcut_text(sc).selected(shown))
                        .clicked()
                    {
                        out.push(Action::TogglePanel(side));
                    }
                    let detached = self.panels.get(side).mode == PanelMode::Detached;
                    let pop = if detached { "Dock" } else { "Pop out" };
                    if ui
                        .button(format!("{pop} {}", label.to_lowercase()))
                        .clicked()
                    {
                        out.push(Action::PopOut(side));
                    }
                }
                if ui
                    .add(egui::Button::new("Toggle both panels").shortcut_text("Ctrl+\\"))
                    .clicked()
                {
                    out.push(Action::ToggleAllPanels);
                }
            });
            ui.separator();
            for t in Tool::ALL {
                let key = format!("{:?}", t.key());
                let selected = self.canvas.tool == t;
                if ui
                    .selectable_label(selected, t.label())
                    .on_hover_text(format!("{} ({key})", t.label()))
                    .clicked()
                {
                    out.push(Action::Tool(t));
                }
            }
        });
        out
    }

    /// The agent indicator and its popup: who is connected, Disconnect, and
    /// the "Agents may edit" toggle (§6.1).
    fn agent_status(&self, ui: &mut egui::Ui) {
        let (Some(host), Some(text)) = (&self.agent, self.agent_indicator()) else {
            return;
        };
        ui.separator();
        let sessions = host.sessions();
        let color = if !host.agents_may_edit() {
            ui.visuals().warn_fg_color
        } else if sessions.is_empty() {
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().hyperlink_color
        };
        let button =
            egui::Button::new(egui::RichText::new(format!("● {text}")).color(color)).frame(false);
        egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
            ui.set_min_width(260.0);
            ui.strong("Agents");
            match (host.socket_path(), host.socket_error()) {
                (Some(p), _) => ui.weak(format!("Listening on {}", p.display())),
                (None, Some(e)) => ui.colored_label(ui.visuals().warn_fg_color, e),
                (None, None) => ui.weak("Not listening on a socket"),
            };
            ui.separator();
            if sessions.is_empty() {
                ui.label("No agents connected.");
                ui.weak("Connect Claude Code with:\nclaude mcp add tessel -- tessel mcp");
            }
            for s in &sessions {
                ui.horizontal(|ui| {
                    ui.label(&s.client);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Disconnect").clicked() {
                            host.disconnect(s.id);
                        }
                    });
                });
            }
            ui.separator();
            let mut may_edit = host.agents_may_edit();
            if ui
                .checkbox(&mut may_edit, "Agents may edit")
                .on_hover_text(
                    "Off: agents can read, select and screenshot, but not change the document",
                )
                .changed()
            {
                host.set_agents_may_edit(may_edit);
            }
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(format!("{:.0}%", self.canvas.view.zoom * 100.0));
            ui.separator();
            match self.canvas.cursor {
                Some(p) => ui.label(format!("{:.0}, {:.0}", p.x, p.y)),
                None => ui.label("–"),
            };
            ui.separator();
            ui.label(format!("rev {}", self.ed.revision));
            self.agent_status(ui);
            if self.ed.read_only {
                ui.separator();
                ui.colored_label(ui.visuals().warn_fg_color, "read-only");
            }
            if let Some(s) = &self.ed.status {
                ui.separator();
                ui.label(s);
            }
        });
    }

    fn problems_banner(&self, ui: &mut egui::Ui) {
        if self.ed.problems.is_empty() {
            return;
        }
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .inner_margin(6.0)
            .show(ui, |ui| {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "This file has {} problem(s), so it is open read-only. Fix them in the file and reopen it.",
                        self.ed.problems.len()
                    ),
                );
                egui::CollapsingHeader::new("Problems").show(ui, |ui| {
                    for p in &self.ed.problems {
                        ui.label(p.to_string());
                    }
                });
            });
    }

    /// Autosave and watch the open file; called every frame.
    fn background_tasks(&mut self, ctx: &egui::Context) {
        let now = self.ed.clock.now();
        if let Some(msg) = self.autosaver.tick(&self.ed, now) {
            self.ed.status = Some(msg);
        }
        match self.watcher.poll(now, self.ed.is_dirty()) {
            Decision::Nothing => {}
            Decision::Reload => self.reload_from_disk(),
            Decision::Ask => self.reload_prompt = true,
        }
        // Wake up for the next autosave or poll even when nothing happens.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn reload_from_disk(&mut self) {
        self.canvas.interrupt(&mut self.ed);
        match watch::reload(&mut self.ed) {
            Ok(()) => {
                self.inspector = Inspector::default();
                self.ed.status = Some(format!(
                    "Reloaded {} (changed on disk)",
                    self.ed.display_name()
                ));
            }
            Err(e) => {
                tracing::warn!("reload: {e:#}");
                self.ed.status = Some(format!("Cannot reload {}: {e}", self.ed.display_name()));
            }
        }
        // Either way this version of the file has been seen.
        self.watcher.sync(self.ed.path.as_deref());
    }

    fn reload_dialog(&mut self, ctx: &egui::Context) {
        if !self.reload_prompt {
            return;
        }
        let mut choice = None;
        egui::Modal::new(egui::Id::new("changed-on-disk")).show(ctx, |ui| {
            ui.heading("File changed on disk");
            ui.label(format!(
                "{} was changed by another program, and you have unsaved edits.",
                self.ed.display_name()
            ));
            ui.horizontal(|ui| {
                if ui
                    .button("Reload")
                    .on_hover_text("Discard my edits")
                    .clicked()
                {
                    choice = Some(true);
                }
                if ui.button("Keep mine").clicked() {
                    choice = Some(false);
                }
            });
        });
        if let Some(reload) = choice {
            self.answer_reload(reload);
        }
    }

    /// Answer the "changed on disk" question: reload (`true`, dropping the
    /// unsaved edits) or keep mine (`false`).
    pub fn answer_reload(&mut self, reload: bool) {
        self.reload_prompt = false;
        if reload {
            self.reload_from_disk();
        } else {
            self.watcher.sync(self.ed.path.as_deref());
        }
    }

    fn recovery_dialog(&mut self, ctx: &egui::Context) {
        if self.recovery_offer.is_none() {
            return;
        }
        let mut choice = None;
        egui::Modal::new(egui::Id::new("recovery")).show(ctx, |ui| {
            ui.heading("Recover unsaved work?");
            ui.label(format!(
                "Tessel found autosaved changes to {} from an earlier session.",
                self.ed.display_name()
            ));
            ui.horizontal(|ui| {
                if ui.button("Restore").clicked() {
                    choice = Some(true);
                }
                if ui.button("Discard").clicked() {
                    choice = Some(false);
                }
            });
        });
        if let Some(restore) = choice {
            self.answer_recovery(restore);
        }
    }

    /// Restore (`true`) or discard (`false`) the offered autosave.
    pub fn answer_recovery(&mut self, restore: bool) {
        let Some(offer) = self.recovery_offer.take() else {
            return;
        };
        if restore {
            match autosave::restore(&offer.file, offer.source.as_deref()) {
                Ok(ed) => {
                    self.replace_editor(ed);
                    self.ed.status = Some("Restored autosaved changes".into());
                }
                Err(e) => {
                    tracing::error!("restore {}: {e:#}", offer.file.display());
                    self.ed.status = Some(format!("Cannot restore the autosave: {e}"));
                }
            }
        } else if let Err(e) = std::fs::remove_file(&offer.file) {
            tracing::warn!("discard {}: {e}", offer.file.display());
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(after) = self.confirm else {
            return;
        };
        let mut choice = None;
        egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
            ui.heading("Unsaved changes");
            ui.label(format!("Save changes to {} first?", self.ed.display_name()));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(1);
                }
                if ui.button("Don't save").clicked() {
                    choice = Some(2);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(0);
                }
            });
        });
        match choice {
            Some(1) => {
                self.confirm = None;
                if self.save(false) {
                    self.proceed(ctx, after);
                }
            }
            Some(2) => {
                self.confirm = None;
                self.proceed(ctx, after);
            }
            Some(_) => self.confirm = None,
            None => {}
        }
    }
}

impl DesignApp {
    /// A panel's contents behind a header with its Pop out / Dock button.
    fn side_contents(&mut self, ui: &mut egui::Ui, side: Side, actions: &mut Vec<Action>) {
        let detached = self.panels.get(side).mode == PanelMode::Detached;
        ui.horizontal(|ui| {
            ui.strong(match side {
                Side::Left => "Document",
                Side::Right => "Inspector",
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, tip) = if detached {
                    ("Dock", "Put this panel back in the main window")
                } else {
                    ("Pop out", "Show this panel in its own window")
                };
                if ui.small_button(label).on_hover_text(tip).clicked() {
                    actions.push(Action::PopOut(side));
                }
            });
        });
        match side {
            Side::Left => self.left_panel(ui),
            Side::Right => {
                self.instance_inspector.ui(ui, &self.ed);
                self.inspector.ui(ui, &self.ed);
            }
        }
    }

    /// One side panel, docked in the main window or in its own viewport.
    fn side_panel(&mut self, ui: &mut egui::Ui, side: Side, actions: &mut Vec<Action>) {
        let (id, title, default_width, default_size) = match side {
            Side::Left => ("layers", "Layers", 240.0, [260.0, 700.0]),
            Side::Right => ("inspector", "Inspector", 280.0, [300.0, 700.0]),
        };
        match self.panels.get(side).mode {
            PanelMode::Hidden => {}
            PanelMode::Docked => {
                let width = self.panels.get(side).docked_width.unwrap_or(default_width);
                let panel = match side {
                    Side::Left => egui::Panel::left(id),
                    Side::Right => egui::Panel::right(id),
                };
                let resp = panel
                    .resizable(true)
                    .default_size(width)
                    .show(ui, |ui| self.side_contents(ui, side, actions));
                let w = resp.response.rect.width();
                let p = self.panels.get_mut(side);
                if p.docked_width.is_none_or(|old| (old - w).abs() > 0.5) {
                    p.docked_width = Some(w);
                }
            }
            PanelMode::Detached => {
                let ctx = ui.ctx().clone();
                let size = self.panels.get(side).window_size.unwrap_or(default_size);
                let builder = egui::ViewportBuilder::default()
                    .with_app_id(APP_ID)
                    .with_title(format!("{title} \u{2014} Tessel"))
                    .with_inner_size(size)
                    .with_min_inner_size([200.0, 200.0]);
                let vid = egui::ViewportId::from_hash_of(("panel", id));
                let mut closed = false;
                ctx.show_viewport_immediate(vid, builder, |ui, _class| {
                    let vctx = ui.ctx().clone();
                    // Shortcuts work in this window too; they run with the
                    // main window's actions.
                    actions.extend(self.shortcuts(&vctx));
                    egui::CentralPanel::default().show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .show(ui, |ui| self.side_contents(ui, side, actions));
                    });
                    if let Some(r) = vctx.input(|i| i.viewport().inner_rect) {
                        let s = [r.width(), r.height()];
                        let p = self.panels.get_mut(side);
                        let moved = |a: f32, b: f32| (a - b).abs() > 0.5;
                        if p.window_size
                            .is_none_or(|o| moved(o[0], s[0]) || moved(o[1], s[1]))
                        {
                            p.window_size = Some(s);
                        }
                    }
                    if vctx.input(|i| i.viewport().close_requested()) {
                        closed = true;
                    }
                    // Edits here show up on the main canvas.
                    if vctx.input(|i| !i.events.is_empty()) {
                        vctx.request_repaint_of(egui::ViewportId::ROOT);
                    }
                });
                if closed {
                    self.panels.close_detached(side);
                }
            }
        }
    }
}

impl eframe::App for DesignApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Detached panels are viewports of their own; only the root window's
        // close request means quit.
        if ctx.viewport_id() == egui::ViewportId::ROOT
            && ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
        {
            if self.ed.is_dirty() {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.confirm = Some(After::Quit);
            } else {
                self.discard_recovery();
            }
        }
        if let Some(host) = &mut self.agent {
            host.process(&ctx, frame, &mut self.ed, &mut self.canvas);
        }

        let mut actions = self.shortcuts(&ctx);
        egui::Panel::top("menu").show(ui, |ui| {
            actions.extend(self.menu_bar(ui));
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("activity").show(ui, |ui| {
            let now = self.ed.clock.now();
            self.activity.ui(ui, &self.ed.history, now);
        });
        self.side_panel(ui, Side::Left, &mut actions);
        self.side_panel(ui, Side::Right, &mut actions);
        self.instance_inspector.apply(&ctx, &mut self.ed);
        if self.inspector.has_changes() {
            // An inspector edit ends any canvas drag or inline text edit.
            self.canvas.interrupt(&mut self.ed);
        }
        self.inspector.apply(&ctx, &mut self.ed);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.problems_banner(ui);
                self.components.master_bar(ui, &mut self.ed);
                self.components.sync_view(&mut self.canvas, &self.ed);
                self.canvas.ui(ui, frame, &mut self.ed);
            });

        for a in actions {
            self.run_action(&ctx, a);
        }
        match self.export.ui(&ctx, &self.ed) {
            Some(Request::Copy(text)) => {
                ctx.copy_text(text);
                self.ed.status = Some("Copied to the clipboard".into());
            }
            Some(Request::Save) => self.export_save_dialog(frame),
            None => {}
        }
        self.recovery_dialog(&ctx);
        self.reload_dialog(&ctx);
        self.confirm_dialog(&ctx);
        self.components.dialogs(&ctx, &mut self.ed);
        self.background_tasks(&ctx);
        if let Some(host) = &mut self.agent {
            host.end_frame(&self.ed);
        }

        self.save_panels(&ctx);
        let t = title(&self.ed);
        if t != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(t.clone()));
            self.title = t;
        }
    }
}
