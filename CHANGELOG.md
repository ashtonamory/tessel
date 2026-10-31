# Changelog

## Unreleased

- **Headless.** `tessel render FILE` renders a design file to PNG, JPEG or
  SVG with no window, with an optional layer map (`--layers`) of every
  visible layer's box in the image's pixels. `tessel mcp --headless --root
  DIR` serves the agent tools on the design files in one folder, adds
  `list_documents`, `open_document`, `new_document` and `import_image`, and
  saves each edit at once. Both fall back to software rendering (Mesa's
  lavapipe) where no GPU works.

## 0.1.0

The first release. See [docs/SPEC.md](docs/SPEC.md) for the full design.

- **Canvas and editing.** Pan and zoom canvas with rectangle, ellipse, frame
  and text tools; select, marquee, move, resize and rotate; snapping with
  smart guides and distances; groups, z-order, copy, paste and duplicate;
  multiple pages; layers panel with rename, hide, lock and reparent.
- **Inspector.** Position, size, rotation, fills (solid and gradients),
  strokes, corner radius, opacity, drop shadows and text properties, with
  values that can be typed, dragged or bound to a token or style.
- **Images and SVG.** Insert images from a file or the clipboard (clipboard
  paste works on X11 only), stored beside the document by hash; import SVG as
  layers.
- **Tokens, styles and auto-layout.** Color, number, radius and font tokens;
  reusable fill, stroke, text and effect styles; flexbox auto-layout on taffy
  with on-canvas padding and gap handles.
- **Components.** Create components from a selection, place instances with
  overrides, edit a master (all instances update), detach instances.
- **History and recovery.** Undo and redo with labelled steps, an Activity
  panel, autosave and recovery, and reload when the file changes on disk.
- **Agents (MCP).** Each window runs an MCP server on a local unix socket,
  reached through the `tessel mcp` stdio bridge, with 20 tools for reading,
  editing, selecting, screenshots and export, plus the `design_conventions`
  prompt. Each agent call is one undo step; "Agents may edit" makes a
  document read-only to agents.
- **Export.** PNG (1x to 3x) and SVG; code as HTML + CSS, Tailwind v3 or v4,
  or React (TSX); tokens as CSS variables or a Tailwind theme.
- **Hardening.** Strict checks on `export_file` paths and the socket
  directory, limits on asset reads and component expansion, read-only opening
  of documents with problems, and sanitized exported code. See
  [SECURITY.md](SECURITY.md).
- **Desktop integration.** Desktop entry, icon, MIME type for
  `*.tessel.json`, and install and uninstall scripts.
- **Panels.** Dock, hide or pop out the left and right side panels.
