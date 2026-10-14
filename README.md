# Tessel

Tessel is a design tool for Linux (built for Omarchy: Arch + Hyprland on
Wayland). Design app screens and web pages, then hand them to an AI agent to
build in code — or have the agent co-design with you live through the app's
built-in MCP server.

Stack: Rust, eframe/egui for the UI, vello on wgpu for the canvas, taffy for
flexbox auto-layout, and an MCP server for agents such as Claude Code.

Status: v1 is complete: milestones M1 (canvas basics), M2 (inspector, images,
pages, structure, canvas precision, history and recovery), M3 (tokens, styles,
auto-layout), M4 (MCP agent link) and M5 (components, code and token export)
are all merged. See [docs/SPEC.md](docs/SPEC.md) for goals, the document model,
file format, command model, MCP tools and milestones, and
[CHANGELOG.md](CHANGELOG.md) for what landed when.

Crates:

- `crates/doc`: document model, canonical `.tessel.json` reader/writer,
  validation, commands and undo history.
- `crates/layout`: absolute geometry of every node, parley text measurement,
  hit-testing.
- `crates/render`: vello scenes from a document, cached per page-level node;
  rendering on a shared wgpu device, to a canvas texture or offscreen RGBA.
- `crates/export`: PNG (offscreen on the app's GPU device) and SVG export, code
  export (HTML + CSS, Tailwind v3/v4, React TSX) and token export (CSS
  variables, Tailwind theme).
- `crates/agent`: the MCP server (20 tools, the `design_conventions` prompt,
  selection and document resources) on `rmcp` 3.5.1.
- `crates/app`: the `tessel` binary (eframe/egui), which hosts the
  MCP server, and its `tessel mcp` stdio bridge.

## Requirements

Tessel runs on **Linux only**, on Wayland or X11 (it uses unix sockets for the
agent link, so other platforms are not supported). You need a Rust toolchain
(stable, installed with [rustup](https://rustup.rs)), a Vulkan-capable GPU
driver (the canvas draws with vello on wgpu), and these system packages. The
names below are for Debian/Ubuntu and Arch; other distributions have
equivalents.

Debian/Ubuntu:

```sh
sudo apt-get install build-essential pkg-config \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libfontconfig1-dev \
  libx11-dev libxcursor-dev libxi-dev libxrandr-dev libxcb1-dev \
  libvulkan1 mesa-vulkan-drivers
```

Arch (Omarchy), where the development headers ship with the runtime packages:

```sh
sudo pacman -S --needed base-devel pkgconf wayland libxkbcommon fontconfig \
  libx11 libxcursor libxi libxrandr libxcb vulkan-icd-loader \
  xdg-desktop-portal xdg-desktop-portal-gtk
```

Also install a Vulkan driver for your GPU (`vulkan-radeon`, `vulkan-intel` or
`nvidia-utils`) and, for the file dialogs, the portal backend for your
desktop instead of `-gtk` if you prefer (for example
`xdg-desktop-portal-hyprland`). The Vulkan loader and driver are a run-time
requirement on both distributions.

- wgpu: Vulkan (`libvulkan1` and a driver such as `mesa-vulkan-drivers`).
- winit/eframe: `libwayland-dev` and `libxkbcommon-dev` (plus the X11
  libraries for X11 sessions).
- fonts: `libfontconfig1-dev` (fontconfig).
- file dialogs: `rfd` uses the XDG desktop portal, so a running
  `xdg-desktop-portal` with a backend (for example
  `xdg-desktop-portal-gtk` or `-hyprland`/`-kde`) is needed at run time for
  Open and Save.

## Build and run

```sh
git clone <this repository> tessel && cd tessel
cargo run --release -p app -- [FILE.tessel.json]
```

Install it as a desktop app (Omarchy / any freedesktop desktop; the walker
launcher finds it as "Tessel"):

```sh
scripts/install.sh              # to ~/.local; PREFIX=/usr/local for another prefix
scripts/install.sh --dry-run    # print what it would do
scripts/uninstall.sh            # remove it again
```

It builds offline (falling back to a normal build if that fails), installs the
binary, the `.desktop` entry, the icon and the `application/x-tessel`
MIME type (`*.tessel.json`), refreshes the desktop and MIME databases and makes
the app the default opener for those files. `DESTDIR` stages the install
without touching the databases. Make sure `$PREFIX/bin` is on your `PATH`.

## Controls

**Tools** (also in the toolbar): Move `V`, Frame `F`, Rectangle `R`,
Ellipse `O`, Text `T`. `Esc` cancels or deselects.

**Select and edit.** Click selects; Shift adds; Ctrl selects inside groups.
Drag on empty canvas to marquee. Drag handles to resize (Shift keeps aspect,
Alt resizes from the center). Drag just outside a corner handle to rotate (snaps
to 15 degrees). Double-click text to edit it. Arrow keys nudge 1 px, Shift+arrow
10 px. `Delete`/`Backspace` deletes. Ctrl+A selects all.

**Snapping.** Moving, resizing and rotating snap to other layers, the page and
whole pixels, with guides and distances shown. Hold Ctrl while dragging to turn
snapping off.

**View.** Scroll or middle-drag or Space+drag pans; Ctrl+scroll or pinch zooms.
Shift+1 fits, Shift+0 is 100%, Shift+2 zooms to the selection.

**Panels.** The left panel (Layers, Tokens, Styles, Components) and the right
panel (inspector) can each be docked, hidden or popped out into their own
window. View > Left panel / Right panel show or hide them (Ctrl+Shift+L and
Ctrl+Shift+I); Ctrl+\ hides both, or brings both back. The Pop out button in a
panel's header (or View > Pop out ...) moves it to a separate window; closing
that window docks the panel again. Shortcuts and edits work from either window.
The layout and sizes are remembered in `$XDG_CONFIG_HOME/tessel/ui.json`
(default `~/.config/tessel/ui.json`).

**Files.** Ctrl+N new, Ctrl+O open, Ctrl+S save, Ctrl+Shift+S save as, Ctrl+Q
quit, Ctrl+Shift+E opens the Export window. File > Import SVG... turns an SVG into layers. Shift+Ctrl+K inserts an
image (also pasting an image from the clipboard); images are stored beside the
document and referenced by hash.

**History.** Ctrl+Z undo; Ctrl+Shift+Z or Ctrl+Y redo. The Activity panel (under
the layers) lists recent edits, including those an agent made over MCP.

**Structure.** Ctrl+C / Ctrl+X / Ctrl+V copy, cut and paste (paste also takes
images); Ctrl+D duplicates. Ctrl+G groups, Ctrl+Shift+G ungroups, Ctrl+Alt+G
wraps the selection in a frame. Z-order: Ctrl+] forward, Ctrl+[ backward,
Ctrl+Shift+] to front, Ctrl+Shift+[ to back. The Pages list at the left adds,
renames, reorders and deletes pages. The layers panel selects, renames
(double-click), hides, locks and reorders or reparents (drag) layers.

**Inspector** (right): position, size, rotation, layout, fill, stroke, effects
and text for the selection. Values can be typed, dragged, or bound to a token
or style from the picker next to them.

**Tokens and styles.** The left panel has Layers, Tokens and Styles tabs. Tokens
are named colors and numbers; styles are reusable fill, stroke, text and effect
sets. Binding a property to one keeps it in sync; "Unlink token" and "Detach
style" make it a plain value again.

**Auto-layout.** Shift+A turns the selected frame (or the selection) into a
flex auto-layout frame; its direction, gap, padding, alignment and child
sizing are in the inspector, with on-canvas handles for padding and gap.

**Components.** Ctrl+Alt+K turns the selected frame into a component (several
layers are wrapped in a frame first); the frame becomes an instance of it.
Ctrl+Alt+B detaches the selected instances into ordinary frames. The left
panel's Components tab lists components (grouped by `/` in their names): click
one to place an instance in the middle of the view, or drag it onto the canvas.
Each row has buttons to rename, edit and delete (deleting a used component asks
to "Detach and delete"). Select an instance and the inspector shows its
component, "Go to master", "Detach" and its overrides; changing a property of
an instance's inner layer stores an override on the instance. The Components
menu has Create component, Detach instance, Go to master and, while editing,
Leave component.

**Editing a master.** "Edit component" (the ◆ button in the Components tab)
shows the master on its own Components page. Canvas, layers and inspector work
as on any page, a banner reads "Editing component …", and every edit updates
all instances. Leave with "Done" in the banner or `Esc` with nothing selected.
Undo and redo stay inside the component while you edit it.

**Autosave and reload.** Unsaved work is autosaved every 30 s to
`$XDG_STATE_HOME/tessel/recovery` (default `~/.local/state/...`); on the
next open you are offered to restore it. If the file changes on disk (an agent
or another editor), it reloads silently when you have no edits and asks
"Reload / Keep mine" when you do.

## Agents (MCP)

Every window runs an MCP server for coding agents such as Claude Code. It
listens only on a unix socket, `$XDG_RUNTIME_DIR/tessel/<pid>.sock`
(mode 0600 in a 0700 directory, so only you can connect), and
`current.sock` beside it points at the window you focused last. Agents
reach it through `tessel mcp`, a stdio bridge. Register it once with
Claude Code:

```sh
claude mcp add tessel -- tessel mcp
```

or for one project, committed to the repository's `.mcp.json`:

```sh
claude mcp add --scope project tessel -- tessel mcp
```

which writes:

```json
{ "mcpServers": { "tessel": { "command": "tessel", "args": ["mcp"] } } }
```

The bridge connects to the most recently focused window. To pick another,
pass `--file PATH` (the window that has that document open), `--pid PID` or
`--socket PATH`. With `--launch` it starts the app (detached, on `--file` if
given) lazily: not when the agent session starts, but on the first tool call
(or other request that needs a window) when none is running. Without it, the
agent is told the app is not running (`not_running`) until you start it; the
bridge then connects on the next call. `tessel mcp --help` lists the
options.

The agent has 20 tools (`crates/agent`):

- read: `get_document`, `get_node`, `find_nodes`, `get_selection`, `get_tokens`
- edit: `create_node`, `update_node`, `update_nodes`, `delete_nodes`,
  `move_node`, `duplicate_node`, `set_tokens`, `upsert_style`, `apply_style`
- components: `create_component`, `create_instance`
- point: `set_selection` (selects in the app; not an edit)
- look and export: `screenshot` (a layer as the canvas shows it), `export_code`
  (HTML + CSS, Tailwind or React as text) and `export_file` (PNG or SVG; the
  file must end in `.png` or `.svg` to match, sit inside the document's folder
  or your home folder with no hidden folder or file (name starting with `.`)
  below it, and is never written through a symlink; an existing file is only
  replaced if it is a regular file)

Each tool call is one undo step, so one Ctrl+Z undoes a whole agent call. It is
labelled "Agent: …" and shows in the Activity panel tagged with the agent's
name. Locked layers are read-only to agents, and an agent edit to a layer you
are dragging or typing in waits for you to finish (up to 2 s).

The status bar shows how many agents are connected and which. Click it to
disconnect one, or untick "Agents may edit" to make the document read-only to
agents (they can still read, select and screenshot).

## Exporting

Ctrl+Shift+E (File > Export...) opens the Export window for the selected layer. Pick a format: PNG
1x/2x/3x, SVG, HTML + CSS, Tailwind v4, Tailwind v3 or React (TSX + Tailwind).
For code formats choose how tokens appear, as CSS variables or inline values.
The files are previewed; "Copy to clipboard" copies the shown file and "Save to
folder..." writes them all. File > Export tokens writes the document's tokens as
CSS variables, a Tailwind v4 theme or a Tailwind v3 config. Exports are
deterministic. A stroke on a leaf layer with `inside` alignment becomes a CSS
`border`; other strokes become an `outline`, because a border would shift
children.

Two optional browser checks (not part of `cargo test`):

```sh
scripts/layout-parity.sh   # writes HTML twins of the layout fixtures, to open in a browser
scripts/code-parity.sh     # exports HTML for the fixtures, loads it in headless Chromium and compares box geometry
```

`code-parity.sh` needs Chromium (`CHROMIUM=/path/to/chrome` picks one).

Build and test:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to contribute and
[SECURITY.md](SECURITY.md) for the threat model, including a warning about
opening untrusted documents with an agent connected.

GPU tests (render output, the canvas texture, agent screenshots) skip when no
wgpu adapter is found.

## Known limitations

- Linux only (Wayland or X11). There are no macOS or Windows builds.
- Pasting an image from the clipboard works on X11 only, not on Wayland. Use
  the Insert image command (Shift+Ctrl+K) to choose a file there.
- GPU tests skip when no wgpu adapter is found, so a headless machine does not
  exercise rendering.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in Tessel by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any additional
terms or conditions.
