//! The preview window (§13.2): View > Preview, or the agent's
//! `open_preview`, shows a top-level frame's HTML + CSS export in a browser
//! window that reloads after each change.

use std::time::{Duration, Instant};

use doc::NodeId;

use crate::editor::Editor;

/// How long the document must hold still before the preview re-exports.
/// Edits keep coming during a drag, so this also spaces exports out.
pub const SETTLE: Duration = Duration::from_millis(150);

/// The preview server and what it shows. Lives until the app closes.
pub struct PreviewWindow {
    server: preview::Server,
    /// The top-level node shown.
    frame: NodeId,
    /// The editor revision last published, and the frame it showed.
    published: Option<(u64, NodeId)>,
    /// When the document first changed since the last publish.
    changed_at: Option<Instant>,
}

impl PreviewWindow {
    pub fn url(&self) -> &str {
        self.server.url()
    }

    /// Export the frame now and publish it.
    fn publish(&mut self, ed: &Editor) -> Result<(), String> {
        let file = ed.display_name();
        let page = preview::export_page(
            ed.document(),
            ed.document_geometry(),
            &ed.assets,
            &self.frame,
            &file,
            ed.revision,
        )?;
        self.server.publish(page);
        self.published = Some((ed.revision, self.frame.clone()));
        self.changed_at = None;
        Ok(())
    }

    /// Call once a frame: re-export after the document settles. Returns
    /// how soon to look again, if a change is waiting.
    pub fn tick(&mut self, ed: &Editor, now: Instant) -> Option<Duration> {
        let doc = ed.document();
        if doc.node_or_master(&self.frame).is_none() {
            // The frame was deleted or another file opened: follow the page.
            self.frame = preview::pick_frame(doc, ed.document_page(), &ed.selection)?;
        }
        if self.published.as_ref() == Some(&(ed.revision, self.frame.clone())) {
            return None;
        }
        let since = *self.changed_at.get_or_insert(now);
        let waited = now.saturating_duration_since(since);
        if waited < SETTLE {
            return Some(SETTLE - waited);
        }
        if let Err(e) = self.publish(ed) {
            tracing::warn!("preview: {e}");
            self.published = Some((ed.revision, self.frame.clone()));
            self.changed_at = None;
        }
        None
    }
}

/// Show `target`'s top-level frame (default: the selection's, else the
/// page's first) in the preview, starting the server and opening a browser
/// window if none is showing it. Returns the preview's URL.
pub fn open(
    preview: &mut Option<PreviewWindow>,
    ed: &Editor,
    target: Option<&NodeId>,
    profile_dir: &std::path::Path,
) -> Result<String, String> {
    let doc = ed.document();
    let frame = match target {
        Some(id) => preview::top_level_of(doc, id).unwrap_or_else(|| id.clone()),
        None => preview::pick_frame(doc, ed.document_page(), &ed.selection)
            .ok_or("this page has nothing to preview")?,
    };
    if preview.is_none() {
        let server = preview::Server::start().map_err(|e| format!("preview server: {e}"))?;
        *preview = Some(PreviewWindow {
            server,
            frame: frame.clone(),
            published: None,
            changed_at: None,
        });
    }
    let p = preview.as_mut().expect("just set");
    p.frame = frame;
    p.publish(ed)?;
    if p.server.clients() == 0 {
        let program = preview::open_window(p.url(), profile_dir)
            .map_err(|e| format!("could not open a browser: {e}"))?;
        tracing::info!("preview at {} in {program}", p.url());
    }
    Ok(p.url().to_string())
}
