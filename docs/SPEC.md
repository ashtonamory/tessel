# Tessel — v1 specification

Status: v1 is built (milestones M1–M5 below). This spec is kept in sync with
what shipped; change it by PR when the design changes.

Tessel is a design tool for app screens and web pages on Linux, built first for Omarchy
(Arch + Hyprland, Wayland). You use it to design app screens and web pages
before an AI agent builds them in code. The AI can also join the design work:
the app runs an MCP server, so Claude Code can connect to the running app, read
the design, edit it live, screenshot it and export it to code.

Stack (decided):

| Concern            | Choice                                   |
| ------------------ | ---------------------------------------- |
| Language / build   | Rust, one Cargo workspace                |
| App UI (chrome)    | eframe / egui 0.36 on the wgpu backend   |
| Design canvas      | vello on wgpu (shares eframe's device)   |
| Text shaping       | parley (vello's text stack)              |
| Auto-layout        | taffy (flexbox, maps 1:1 to CSS)         |
| Agent interface    | built-in MCP server                      |
| File format        | canonical, pretty-printed JSON           |

---

## 1. Goals and non-goals (v1)

### Goals

1. **Runs well on Hyprland/Wayland.** Native Wayland via winit, fractional
   scaling correct, a stable `app_id` (`tessel`) so users can write
   window rules. Works under XWayland as a fallback, but Wayland is the target.
2. **Fast canvas.** Smooth pan and zoom (target: one frame per vsync at
   120 Hz) on documents with a few thousand nodes.
3. **A design model that maps to code.** Frames with auto-layout are flexbox.
   Sizing modes are fixed / hug / fill, the same three things CSS and native
   layout systems express. What you see is what the exported HTML renders.
4. **Agent co-design.** An AI agent connected over MCP can do anything the
   user can do to the document, sees what the user has selected, and can
   screenshot frames to check its own work. Agent edits show up live and are
   undoable like any user edit.
5. **Files that live in git.** The document is a text file with canonical
   ordering, so diffs are small and readable, and an agent can read the file
   directly even when the app is not running.
6. **Design tokens.** Named colors, type styles and spacing values, applied by
   reference, exported as CSS variables / Tailwind theme.
7. **Components and instances**, with overrides, exported as React components.
8. **Export** frames to PNG and SVG, and to HTML+CSS, HTML+Tailwind and React
   (TSX + Tailwind) code.

### Non-goals (v1)

- Multiplayer, real-time collaboration between humans, or any server/cloud.
  (One user plus agents connected to their local app is the only "multi" case.)
- A plugin system or scripting API beyond MCP.
- Prototyping: links between frames, transitions, interactive states.
- Boolean operations on vectors, a full pen tool with path editing beyond
  basic point editing, and vector networks.
- Importing files from other design tools.
- Masks, blend modes beyond normal, and blur effects (drop shadow is in).
- Variable fonts axes UI, OpenType feature UI, text on a path.
- Version history inside the app (git is the version history).
- Windows and macOS builds. Nothing should block them, but they are untested.
- Accessibility of the app's own UI beyond what egui gives (AccessKit).

---

## 2. Crate layout

One Cargo workspace. Library crates are small and have one job each; only
`app` knows about windows.

```
tessel/
├── Cargo.toml              # [workspace], shared [workspace.dependencies]
├── crates/
│   ├── doc/                # document model, file format, commands, history
│   ├── layout/             # doc -> taffy -> computed geometry
│   ├── render/             # doc + geometry -> vello Scene; offscreen PNG
│   ├── export/             # PNG, SVG, HTML/CSS, Tailwind, React
│   ├── agent/              # MCP server (tools, resources, transports)
│   └── app/                # eframe shell; binary `tessel`
└── docs/SPEC.md
```

Dependency direction (no cycles; arrows mean "depends on"):

```
app ──► agent ──► export ──► render ──► layout ──► doc
 │        │         │           ▲          ▲
 └────────┴─────────┴───────────┴──────────┘ (all may use doc)
```

| Crate    | Responsibility | Key deps | Must not depend on |
| -------- | -------------- | -------- | ------------------ |
| `doc`    | Pure data: `Document`, nodes, styles, components, IDs. Serde (de)serialization and canonical JSON writer. `Command` enum, `apply`, inverse generation, `History`. Validation (tree invariants). Format migrations. | `serde`, `serde_json`, `uuid` (or `getrandom`) for ID entropy | anything GPU, egui, tokio, taffy |
| `layout` | Build/maintain a taffy tree from the document, run layout, produce `Geometry` (absolute bounds + transform per node). Text measurement via parley. | `doc`, `taffy`, `parley` | GPU, egui |
| `render` | Turn `doc` + `Geometry` into a `vello::Scene`. Image cache, font/glyph handling, scene caching per top-level frame. Offscreen render of one node to RGBA (for screenshots and PNG export). Hit-test helpers (kurbo shapes). | `layout`, `vello`, `image` | egui |
| `export` | PNG (via `render` offscreen + `png`), SVG (hand-written writer), HTML+CSS, HTML+Tailwind, React TSX. Token export (CSS vars, Tailwind theme). Pure functions `fn(&Document, &Geometry, NodeId, Options) -> Output`. | `doc`, `layout`, `render` (PNG only) | egui |
| `agent`  | MCP server: tool definitions + JSON schemas, request handling, unix-socket listener, stdio bridge. Talks to the app only through an `AppHandle` trait (submit command, read snapshot, get selection, render screenshot), so it is testable without a window. | `doc`, `export`, `tokio`, `serde_json`, MCP crate (`rmcp`), `schemars` | egui, `app` |
| `app`    | eframe `App`: toolbars, layers panel, inspector, canvas widget (vello texture inside egui), tools (select, frame, rect, ellipse, text, image, hand), keyboard shortcuts, file dialogs, owns the `Document` + `History` + selection. Implements `AppHandle` for `agent`. Also the `mcp` subcommand (stdio bridge). | everything above, `eframe`, `egui`, `egui-wgpu`, `wgpu`, `rfd` | — |

One binary: `tessel`.

- `tessel [FILE]` opens the app.
- `tessel mcp` is the stdio⇄socket bridge Claude Code launches (§6.1).

---

## 3. Document model

All types live in `doc`. They are plain data: `Clone`, `Debug`, `PartialEq`,
`Serialize`, `Deserialize`. No GPU handles, no caches, no computed geometry.

### 3.1 Units and coordinates

- Units are CSS pixels (logical px). `f32` everywhere.
- Each node's `x`, `y` are relative to its parent's top-left. Pages have no
  origin offset; top-level frames are placed on an infinite page canvas.
- Rotation is degrees, clockwise, around the node's center.
- Colors are sRGB, stored as `#rrggbb` or `#rrggbbaa` strings.
- Values written to disk are rounded to 2 decimal places (§4.3).

### 3.2 IDs

Every node, page, style and component has a stable ID that never changes for
the life of the object, survives save/load, copy within a document (copies get
new IDs), and is what agents use to refer to things.

- Format: a type prefix plus 10 Crockford base32 chars of randomness
  (50 bits), e.g. `n_8k2f9q4m1x` (node), `p_…` (page), `s_…` (style),
  `c_…` (component). The prefix makes IDs self-describing in diffs and agent
  transcripts; random (not sequential) IDs avoid collisions when two git
  branches each add nodes.
- Commands that mint IDs while applying (`CreateComponent`, `DetachInstance`)
  derive them from the document and their arguments, not from fresh entropy,
  so redo recreates the same IDs and later history entries still refer to the
  right objects.
- Generated in `doc::Id::new(kind)`; parse/validate with `Id::parse`.
- IDs are unique across the whole document. Duplicates are a load error.
- Instance children use derived IDs `<instanceId>/<componentNodeId>` so an
  override can name a node inside an instance without copying the subtree
  (§3.7).

### 3.3 Top level

```rust
pub struct Document {
    pub format: FormatHeader,           // { name: "tessel", version: 1 }
    pub meta: Meta,                     // title, created_with app version
    pub tokens: Tokens,                 // §3.6
    pub styles: Vec<Style>,             // §3.6
    pub components: Vec<Component>,     // §3.7
    pub pages: Vec<Page>,               // ordered as in the page list
}

pub struct Page {
    pub id: PageId,
    pub name: String,
    pub background: Color,              // canvas color
    pub children: Vec<Node>,            // top-level nodes, back-to-front
}
```

The tree is stored nested (each node owns its `children`), which is what the
file looks like (§4). In memory, `doc` also keeps a derived
`HashMap<NodeId, NodePath>` index rebuilt after each command so lookups by ID
are O(1); the index is not serialized.

### 3.4 Nodes

```rust
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub visible: bool,                  // default true
    pub locked: bool,                   // default false
    pub x: f32, pub y: f32,             // ignored when positioned by auto-layout
    pub width: f32, pub height: f32,    // current/fixed size; see sizing
    pub rotation: f32,                  // default 0
    pub opacity: f32,                   // 0..=1, default 1
    pub layout_child: LayoutChild,      // how this node sits in an auto-layout parent
    pub constraints: Constraints,       // how it sits in a non-auto-layout frame on parent resize
    pub kind: NodeKind,
}

pub enum NodeKind {
    Frame(Frame),
    Group(Group),
    Rect(Rect),
    Ellipse(Ellipse),
    Text(Text),
    Image(Image),
    Vector(Vector),
    Instance(Instance),
}
```

Kinds:

| Kind      | Has children | What it is | Maps to (code export) |
| --------- | ------------ | ---------- | --------------------- |
| `frame`   | yes | A box with fills, strokes, corner radius, shadow, optional clipping, optional auto-layout. Top-level frames are "screens"/artboards. | `<div>` (or `<section>`/`<button>` via `semantic` hint) |
| `group`   | yes | Pure grouping; its bounds are the union of its children; no styling of its own except opacity. | flattened (children emitted in place) |
| `rect`    | no  | Rectangle with fills, strokes, per-corner radius, shadow. | `<div>` |
| `ellipse` | no  | Ellipse with fills, strokes. | `<div>` with `border-radius: 50%` |
| `text`    | no  | A text block: content, type style, alignment, auto-width / auto-height / fixed sizing. | `<p>`/`<span>`/`<h1-6>` via `semantic` hint |
| `image`   | no  | Raster image fill by asset reference; fit: `fill`, `fit`, `crop`, `tile`. | `<img>` with `object-fit` |
| `vector`  | no  | A path (SVG path data string) with fills and strokes. Icons, lines. | inline `<svg>` |
| `instance`| yes (derived) | A placed copy of a component, with overrides. | `<ComponentName />` (React) / expanded markup (HTML) |

Shared paint/shape props (on frame, rect, ellipse, vector; text has fills only):

```rust
pub struct Shape {
    pub fills: Vec<Paint>,              // painted bottom-to-top
    pub strokes: Vec<Paint>,
    pub stroke_width: f32,
    pub stroke_align: StrokeAlign,      // inside | center | outside
    pub corner_radius: CornerRadius,    // one RadiusRef or [tl, tr, br, bl] of them; rect & frame only
    pub shadows: Vec<Shadow>,           // drop shadows; inner shadow later
    pub fill_style: Option<StyleId>,    // if set, fills come from the style (§3.6)
    pub stroke_style: Option<StyleId>,
    pub effect_style: Option<StyleId>,
}

pub enum Paint {
    Solid { color: ColorRef },
    LinearGradient { start: [f32; 2], end: [f32; 2], stops: Vec<Stop> },  // unit-box coords
    RadialGradient { center: [f32; 2], radius: [f32; 2], stops: Vec<Stop> },
    Image { asset: AssetRef, fit: ImageFit },
}

pub enum ColorRef { Literal(Color), Token(TokenId) }   // "#ff0000" or "{color.primary}"
pub enum RadiusRef { Px(f32), Token(TokenId) }         // 16 or "{radius.md}" (§3.6)
```

Frame-specific:

```rust
pub struct Frame {
    pub shape: Shape,
    pub clip: bool,                     // clip children to bounds; default true
    pub layout: Option<AutoLayout>,     // None = children positioned absolutely
    pub semantic: Option<String>,       // export hint: "button", "nav", "section", …
    pub children: Vec<Node>,
}
```

Text-specific:

```rust
pub struct Text {
    pub content: String,                // plain text; \n for line breaks
    pub fills: Vec<Paint>,
    pub style: TextStyle,               // inline values
    pub text_style: Option<StyleId>,    // if set, overrides `style`
    pub align: TextAlign,               // left | center | right | justify
    pub vertical_align: VAlign,         // top | middle | bottom
    pub sizing: TextSizing,             // auto_width | auto_height | fixed
    pub semantic: Option<String>,       // "h1", "p", "label", …
}

pub struct TextStyle {
    pub font_family: String,            // e.g. "Inter"
    pub font_weight: u16,               // 100..=900
    pub italic: bool,
    pub font_size: f32,
    pub line_height: LineHeight,        // auto | px(f32) | percent(f32)
    pub letter_spacing: f32,            // px
    pub text_transform: TextTransform,  // none | upper | lower | title
    pub decoration: TextDecoration,     // none | underline | strike
}
```

Rich text (mixed styles inside one text node) is **out of v1**; one style per
text node. The model leaves room for a `spans` field later.

### 3.5 Auto-layout

Auto-layout is a frame property and is deliberately a subset of CSS flexbox,
so `layout` is a thin translation to taffy and `export` a thin translation to
CSS.

```rust
pub struct AutoLayout {
    pub direction: Direction,           // row | column           -> flex-direction
    pub wrap: bool,                     //                        -> flex-wrap
    pub gap: SpaceRef,                  // main-axis gap          -> gap / column-gap
    pub cross_gap: Option<SpaceRef>,    // between wrapped lines  -> row-gap
    pub padding: Padding,               // [top, right, bottom, left], each SpaceRef
    pub justify: Justify,               // start | center | end | space_between -> justify-content
    pub align: Align,                   // start | center | end | stretch | baseline -> align-items
    pub width: Sizing,                  // the frame's own sizing on each axis
    pub height: Sizing,
}

pub enum Sizing { Fixed, Hug, Fill }

pub struct LayoutChild {
    pub position: ChildPosition,        // flow (default) | absolute
    pub width: Sizing,                  // when the parent has auto-layout
    pub height: Sizing,
    pub align_self: Option<Align>,
    pub min_width: Option<f32>, pub max_width: Option<f32>,
    pub min_height: Option<f32>, pub max_height: Option<f32>,
}

pub enum SpaceRef { Px(f32), Token(TokenId) }
```

Sizing semantics (the core of the design-to-code mapping):

| Sizing | Meaning | taffy | CSS |
| ------ | ------- | ----- | --- |
| `fixed` | use `width`/`height` | `Dimension::Length(w)` | `width: Wpx` |
| `hug`   | shrink to content | `Dimension::Auto`, no grow | `width: fit-content` (or omitted) |
| `fill`  | take remaining space on the main axis; stretch on the cross axis | main: `flex_grow: 1, flex_basis: 0`; cross: `align_self: stretch` | main: `flex: 1 1 0`; cross: `align-self: stretch` |

Rules:

- A child of an auto-layout frame with `position: flow` ignores its stored
  `x`/`y`; its geometry comes from layout. Its stored `width`/`height` are
  updated to the computed size when the command that caused the change commits,
  so the file always shows the size the user sees (stable, not jittery: only
  written when the rounded value changes).
- `position: absolute` children keep `x`/`y` and use `constraints`
  (CSS `position: absolute` with left/right/top/bottom).
- In frames **without** auto-layout, children are absolute and `constraints`
  (`left | right | left_right | center | scale` per axis) decide what happens
  when the frame resizes.
- Text nodes are taffy leaves with a measure function backed by parley; the
  `sizing` of the text (`auto_width` = hug both, `auto_height` = fixed width,
  hug height) maps onto the same three modes.
- Groups inside auto-layout are treated as a fixed-size leaf (their bounds).
  The UI nudges users toward frames instead.

### 3.6 Tokens and styles

Two layers, matching how design systems and code both work:

**Tokens** are named primitive values. They export as CSS custom properties and
a Tailwind theme.

```rust
pub struct Tokens {
    pub colors: BTreeMap<TokenId, Color>,     // "color.primary" -> "#2563eb"
    pub spacing: BTreeMap<TokenId, f32>,      // "space.4" -> 16
    pub radii: BTreeMap<TokenId, f32>,        // "radius.md" -> 8
    pub fonts: BTreeMap<TokenId, String>,     // "font.sans" -> "Inter"
}
```

Token IDs are dotted, human-chosen names (`color.primary`, `space.4`), not
random IDs, because they appear in code. Renaming a token is a command that
rewrites every reference. A token reference in the file is written
`"{color.primary}"`.

**Styles** are named bundles applied to nodes by reference:

```rust
pub struct Style {
    pub id: StyleId,
    pub name: String,                   // "Heading/H1", "Surface/Card"; '/' groups in UI
    pub kind: StyleKind,
}
pub enum StyleKind {
    Fill { fills: Vec<Paint> },
    Stroke { strokes: Vec<Paint>, width: f32 },
    Text { style: TextStyle },
    Effect { shadows: Vec<Shadow> },
}
```

Editing a style or token updates every node using it. Detaching a style copies
its values onto the node and clears the reference.

### 3.7 Components and instances

```rust
pub struct Component {
    pub id: ComponentId,
    pub name: String,                   // "Button/Primary"; becomes the React name `ButtonPrimary`
    pub root: Node,                     // a frame; the master definition
    pub props: Vec<ComponentProp>,      // exposed overridable things (named), for code export
}

pub struct Instance {
    pub component: ComponentId,
    pub overrides: BTreeMap<String, Override>,  // key: component node id inside root
}

pub enum Override {
    Text { content: String },
    Fills { fills: Vec<Paint> },
    Visible { visible: bool },
    Swap { component: ComponentId },    // instance swap for nested instances
}

pub struct ComponentProp {
    pub name: String,                   // "label"
    pub target: NodeId,                 // node in root
    pub kind: PropKind,                 // text | visible | swap
}
```

- Components live in `Document.components`, not on a page. The UI shows a
  component's master on a "Components" page view; editing it there edits the
  definition.
- An instance stores only its component reference, its own size/position, and
  overrides. Its children are **derived** at load/edit time by expanding the
  component root and applying overrides. Derived children have IDs
  `<instanceId>/<componentNodeId>` and are read-only except for overridable
  properties.
- Instances may nest components. Cycles (a component containing an instance of
  itself, directly or indirectly) are rejected by validation.
- "Detach instance" converts it into an ordinary frame with fresh IDs.
  The frame keeps the instance's ID; nested instances stay instances, with
  the outer overrides merged into theirs.
- Expansion (`doc::expand`) is one level at a time: an instance stands for
  its component root with the instance's own ID, name, position, size,
  visibility and opacity; root children that are not placed by auto-layout
  follow their constraints from the master's size to the instance's. A
  nested instance keeps its derived ID `<instanceId>/<nestedId>`, so its
  children are `<instanceId>/<nestedId>/<nodeId>`.
- Override keys are relative to the instance holding them: `<nodeId>` for a
  node of the root, `<nestedId>/<nodeId>` for a node inside a nested
  instance. An outer override wins over the nested instance's own. A key
  holds one override; overrides that name no node or do not fit its kind
  are ignored.
- `SetProps` on a derived ID takes only `content`, `fills`, `visible` or
  `component` (one per call) and becomes the matching override on the
  instance stored in the document; other props are read-only.

### 3.8 Assets

Images are not inlined in the JSON. They are stored next to the document in
`<name>.assets/<sha256-prefix>.<ext>` and referenced as
`{ "asset": "a1b2c3d4e5f6.png" }`. The content hash names the file, so
duplicates are stored once and renames never happen. Unreferenced assets are
pruned on save.

### 3.9 Invariants (checked by `doc::validate`)

- IDs unique and well-formed; every reference (style, token, component, asset)
  resolves.
- Only `frame`, `group`, `instance`, and page roots have children.
- Groups are non-empty.
- No component cycles.
- Numbers finite; opacity in 0..=1; sizes ≥ 0.

Loading a file that fails validation opens it read-only with a list of
problems rather than refusing outright (agents and humans hand-edit files).

---

## 4. File format

### 4.1 Layout on disk

```
landing.tessel.json       # the document
landing.assets/           # images referenced by the document (may be absent)
```

Extension `.tessel.json`, so editors and agents treat it as JSON. UTF-8, LF
line endings, trailing newline.

### 4.2 Example

```json
{
  "format": { "name": "tessel", "version": 1 },
  "meta": { "title": "Landing page" },
  "tokens": {
    "colors": { "color.bg": "#ffffff", "color.primary": "#2563eb", "color.text": "#0f172a" },
    "spacing": { "space.2": 8, "space.4": 16, "space.8": 32 },
    "radii": { "radius.md": 8 },
    "fonts": { "font.sans": "Inter" }
  },
  "styles": [
    {
      "id": "s_3j8d0k2m9p",
      "name": "Heading/H1",
      "kind": "text",
      "style": {
        "font_family": "{font.sans}",
        "font_weight": 700,
        "font_size": 48,
        "line_height": { "percent": 110 }
      }
    }
  ],
  "pages": [
    {
      "id": "p_0q1w2e3r4t",
      "name": "Home",
      "background": "#f1f5f9",
      "children": [
        {
          "id": "n_8k2f9q4m1x",
          "name": "Desktop",
          "kind": "frame",
          "x": 0, "y": 0, "width": 1440, "height": 900,
          "fills": [{ "type": "solid", "color": "{color.bg}" }],
          "layout": {
            "direction": "column",
            "gap": "{space.8}",
            "padding": ["{space.8}", "{space.8}", "{space.8}", "{space.8}"],
            "align": "center",
            "width": "fixed", "height": "hug"
          },
          "children": [
            {
              "id": "n_5t6y7v8h9r",
              "name": "Title",
              "kind": "text",
              "x": 0, "y": 0, "width": 640, "height": 53,
              "content": "Design it. Then ship it.",
              "text_style": "s_3j8d0k2m9p",
              "fills": [{ "type": "solid", "color": "{color.text}" }],
              "semantic": "h1"
            }
          ]
        }
      ]
    }
  ]
}
```

### 4.3 Canonical form

The writer is deterministic: the same document always produces the same bytes.
This is what keeps git diffs small and lets tests compare files directly.

- **Key order** is fixed per type, in this order: `id`, `name`, `kind`/`type`,
  then geometry (`x`, `y`, `width`, `height`, `rotation`), then
  kind-specific props in declaration order, then `children` **last** (so a
  node's own props are never separated from its header by a long subtree).
  Implemented by struct field order (serde writes fields in declaration
  order) and `BTreeMap` for all maps (sorted keys). `Node` has a hand-written
  serde impl so its `kind` tag can sit between `name` and the geometry;
  other tagged types use `#[serde(tag = "...")]`. `x`, `y`, `width` and
  `height` are always written.
- **Defaults are omitted** (`skip_serializing_if`): `visible: true`,
  `locked: false`, `rotation: 0`, `opacity: 1`, empty arrays, `None`, and
  enum/style values equal to their default (`"auto"`, `"none"`, `"left"`, …).
  Readers fill them back in. This keeps the file short for agents to read.
  Values are compared to the default *after* rounding (opacity `0.999` is
  omitted, not written as `1`), otherwise the next save would differ.
- **Numbers** are rounded to 2 decimals and written in shortest form
  (`16`, not `16.0`; `0.5`, not `0.50`). `-0` is written as `0`.
- **Indentation**: 2 spaces, LF, trailing newline. The root object is always
  expanded. Any other object or array goes on one line when it nests at most
  one level of containers (`[{ "type": "solid", "color": "#fff" }]` qualifies,
  `{ "a": { "b": [1] } }` does not), has no `id` key (nodes, pages, styles and
  components are always expanded), and the line, including indentation, key
  and trailing comma, fits in 100 columns. Everything else is one entry per
  line, except that a run of scalar geometry keys (`x`, `y`, `width`,
  `height`, `rotation`) in an expanded object shares one line. A custom
  `serde_json::ser::Formatter` records the value as a tree, and a small
  layout pass applies these rules. The example in §4.2 is in canonical form
  (a test checks it against `crates/doc/tests/fixtures`).
- **Arrays** keep document order (children are z-order back-to-front; pages
  are page-list order). They are never sorted.
- Unknown keys are preserved on load-save round trips in an `extra` map per
  object, so files written by a newer minor version are not silently damaged.
  They are written after the known props (before `children`, `root` or
  `pages`), sorted, with their values verbatim (floats in shortest form but
  not rounded). Unknown keys on a node belong to its kind's struct
  (`Frame::extra`, …).

### 4.4 Versioning

`format.version` is an integer. A newer major version than the app supports is
refused with a clear message. Older versions are migrated in `doc::migrate` by
a chain of `fn(serde_json::Value) -> serde_json::Value` steps, one per
version, before typed deserialization.

### 4.5 Saving

- Atomic: write to `<file>.tmp`, fsync, rename.
- Autosave of unsaved changes to `$XDG_STATE_HOME/tessel/recovery/`
  every 30 s; offered on next launch after a crash.
- The app watches the file (polls its mtime every second, which needs no new
  crate; `notify`/inotify later if needed) and, if it changes on disk while there are no unsaved edits,
  reloads it. With unsaved edits it asks. This makes "agent edits the file
  directly with a text editor tool" work, though MCP is the preferred path.

---

## 5. Edit / command model

Every change to the document — from a mouse drag, the inspector, a keyboard
shortcut, a paste, or an MCP tool call — is a `Command`. Nothing else mutates
`Document`. This gives one undo stack that covers agent edits, one place to
validate, and one place to notify the renderer and the agent of changes.

### 5.1 Commands

```rust
pub enum Command {
    CreateNode   { parent: ParentRef, index: Option<usize>, node: Node },  // node may carry a subtree
    DeleteNode   { id: NodeId },
    MoveNode     { id: NodeId, parent: ParentRef, index: Option<usize> },  // reparent / reorder
    SetProps     { id: NodeId, patch: NodePatch },                        // any subset of props
    SetGeometry  { id: NodeId, x: f32, y: f32, width: f32, height: f32, rotation: f32 },
    CreatePage   { index: Option<usize>, page: Page },
    DeletePage   { id: PageId },
    UpdatePage   { id: PageId, name: Option<String>, background: Option<Color> },
    UpsertTokens { patch: TokensPatch },                                   // add/change/remove
    RenameToken  { from: TokenId, to: TokenId },                           // rewrites references
    UpsertStyle  { style: Style },                                         // replace by id, or append
    InsertStyle  { index: usize, style: Style },                           // undoes DeleteStyle
    DeleteStyle  { id: StyleId, detach: bool },
    CreateComponent { from: NodeId },                                      // node -> component + instance in place
    UpsertComponent { component: Component },                              // replace by id, or append
    InsertComponent { index: usize, component: Component },                // undoes DeleteComponent
    DeleteComponent { id: ComponentId, detach: bool },                    // refused while used, unless detaching
    DetachInstance  { id: NodeId },                                        // instance -> ordinary frame subtree
    ReplaceNode     { id: NodeId, node: Node },                            // undoes DetachInstance
    Batch(Vec<Command>),
}

pub enum ParentRef { Page(PageId), Node(NodeId) }
```

`NodePatch` is a struct of `Option`s mirroring `Node` (and the per-kind props),
deserializable from a JSON object, so an MCP `update_node` call maps directly
onto `SetProps`. Unknown keys in a patch are an error, not ignored, so agents
find out about typos; so is a known key that does not apply to the node's
kind (`content` on a rect). For optional props (`text_style`, `layout`,
`semantic`, `fill_style`, …) `null` clears the value. An instance's
`overrides` patch is per key: `{ "<key>": { …override… } }` sets one,
`{ "<key>": null }` clears it, and keys not named are kept. A corner radius in
a patch is a `RadiusRef` (px or `"{radius.md}"`), or four of them.

`InsertStyle`, `InsertComponent` and `ReplaceNode` exist as exact inverses (of
`DeleteStyle`, `DeleteComponent` and `DetachInstance`); the UI and agents
rarely issue them directly.

### 5.2 Applying and inverting

```rust
impl Command {
    /// Apply to the document. On success returns the inverse command.
    /// On error the document is unchanged.
    fn apply(self, doc: &mut Document) -> Result<Command, CommandError>;
}
```

- Every command produces its exact inverse while applying (e.g. `DeleteNode`
  returns a `CreateNode` holding the removed subtree at its old index;
  `SetProps` returns a `SetProps` with the previous values of exactly the
  patched fields).
- `Batch` applies in order and rolls back already-applied parts if one fails,
  so a batch is atomic. Its inverse is the reversed batch of inverses.
- After apply, local validation checks invariants around the touched
  nodes. A violation rolls back and returns an error. Only problems the
  command introduced count, so a node that was already invalid (a hand-edited
  file) can still be edited and fixed. Rollbacks and undo/redo only restore
  states the document was already in, so they skip this check.
- Property test (in `doc`): for random documents and random valid commands,
  `apply(c)` then `apply(inverse)` yields a document equal to the original,
  and serialization is byte-identical.

### 5.3 Transactions and history

```rust
pub struct Transaction {
    pub label: String,                  // "Move 3 layers", "Agent: create pricing section"
    pub origin: Origin,                 // User | Agent { client: String }
    pub forward: Vec<Command>,
    pub inverse: Vec<Command>,          // applied in reverse to undo
    pub selection_before: Selection,
    pub selection_after: Selection,
    pub at: Instant,
}

pub struct History { undo: Vec<Transaction>, redo: Vec<Transaction>, limit: usize /* 500 */ }
```

- **One linear history** for the document, shared by user and agents. Ctrl+Z
  undoes the most recent transaction whatever its origin. The undo menu item
  shows the label, so "Undo Agent: create pricing section" is clear.
- **Each MCP tool call is one transaction** (all commands it issues are one
  `Batch`). So one agent action is one undo step.
- **Gestures coalesce.** A drag/resize applies preview commands live, then
  commits one transaction on mouse-up. Typing in an inspector field commits on
  Enter/blur. Consecutive arrow-key nudges within 1 s merge.
- Any new transaction clears the redo stack.
- The history also feeds a small "Activity" list in the UI, where agent
  transactions are tagged, so the user can see what the agent did.

### 5.4 Threading and conflicts

- The `Document` and `History` are owned by the UI thread (the eframe `App`).
- The MCP server runs on a tokio runtime on its own thread. A tool call is
  turned into a request message sent over a channel to the UI thread, with a
  oneshot channel for the reply; the server calls `ctx.request_repaint()` so
  the UI wakes. The UI thread drains agent requests at the start of each frame,
  applies them as transactions, and replies.
- **Reads** (`get_document`, `get_node`, `get_selection`) are served from an
  `Arc<Snapshot>` (document + geometry + selection) the UI thread publishes
  after each committed transaction, so reads never wait for a frame.
- **Conflict with an in-progress user gesture**: if the agent's command
  touches a node the user is currently dragging/editing, the agent request
  waits (up to 2 s) until the gesture commits, then applies against the new
  state. If it would still fail (e.g. the node was deleted), the tool returns
  an error the agent can act on. Agent commands never silently clobber a
  gesture.
- **Locked nodes**: agents can read but not edit nodes the user has locked;
  the tool returns `locked` errors. This gives the user a simple "don't touch
  this" control.

### 5.5 Change notification

After each transaction, the app records the set of changed node IDs. That set
drives: `layout` (re-layout the affected subtrees), `render` (invalidate the
cached scenes of affected top-level frames), the layers panel, and MCP
resource-updated notifications (§6.5).

---

## 6. MCP server

The app hosts an MCP server (protocol revision 2025-06-18 or newer, whatever
the chosen crate supports) that exposes the open document as tools and
resources. It is on by default and listens only locally.

### 6.1 Transport and how Claude Code connects

Claude Code starts MCP servers itself over **stdio**, or connects to them over
**HTTP**. The app, though, is a long-running GUI process the user starts; it
must not be a child of Claude Code. So:

1. **The app listens on a unix socket**:
   `$XDG_RUNTIME_DIR/tessel/<pid>.sock`, mode `0600`, plus a symlink
   `current.sock` to the most recently focused window. The socket speaks MCP's
   stdio framing (newline-delimited JSON-RPC 2.0). File permissions on the
   runtime dir are the authentication: only the same user can connect.
   Without `$XDG_RUNTIME_DIR` the directory is `<tmp>/tessel-$USER`,
   a path another local user could create first, so the app will not bind and
   the bridge will not connect unless the directory is a real directory (not
   a symlink) owned by the current uid with no group or other permission bits.
   `--socket PATH` gets the same check on its parent directory.
2. **`tessel mcp` is a stdio bridge.** Claude Code launches it as a
   normal stdio MCP server. It connects to the socket and copies bytes both
   ways. It holds no state and understands nothing about MCP, except one case:
   if no app is running it answers `initialize` itself and returns a tool
   error on any call ("tessel is not running; start it, or pass
   `--launch`"). With `--launch` it starts the app detached and waits for the
   socket.
   Options: `--socket PATH`, `--pid PID`, `--file PATH` (connect to the window
   that has that document open).
3. Register it once:

   ```sh
   claude mcp add tessel -- tessel mcp
   # or per project, committed to the repo's .mcp.json:
   claude mcp add --scope project tessel -- tessel mcp
   ```

   which writes:

   ```json
   { "mcpServers": { "tessel": { "command": "tessel", "args": ["mcp"] } } }
   ```

Each bridge connection is a separate MCP session. Several agents may connect
at once; their edits are serialized by the UI thread (§5.4) and each is
labeled with the client name from `initialize` (`Origin::Agent { client }`).

**Later (not v1):** an optional streamable-HTTP listener on `127.0.0.1` with a
bearer token, for agents that cannot spawn a local process. Off by default.

The status bar shows a connection indicator: number of connected agents and
their client names; clicking it lists them with a "disconnect" action and an
"agents may edit" toggle (read-only mode).

### 6.2 Conventions for all tools

- IDs are the stable IDs from §3.2. Instance-derived IDs (`n_x/n_y`) are
  accepted where an override is allowed.
- Node props in arguments and results use exactly the file-format JSON (§4),
  so what an agent reads from a file or `get_node` is what it can write back.
- Every mutating tool runs as one transaction and returns the IDs it created
  or touched plus a `revision` number (monotonic, bumped per transaction).
  Mutating tools accept optional `if_revision`: if the document has moved on,
  the call fails with `stale_revision` instead of applying.
- Errors are MCP tool results with `isError: true` and a JSON body
  `{ "code": "...", "message": "...", "node_id"?: "..." }`. Codes:
  `not_found`, `invalid_args`, `invalid_patch`, `locked`, `stale_revision`,
  `invariant`, `read_only`, `not_running`, `internal`.
- Output size: tree reads default to a shallow depth and summarise deeper
  levels (`"children": 12` instead of the array) so results stay small.
- Every tool has a JSON Schema generated with `schemars` from its Rust
  argument type, so the schema and the code cannot drift.

### 6.3 Tools

Types used below: `Id` = string, `Patch` = partial node JSON (§4),
`NodeSpec` = node JSON without `id` (IDs are assigned; a nested `children`
array creates a whole subtree in one call).

**Read**

| Tool | Arguments | Returns |
| ---- | --------- | ------- |
| `get_document` | `{ page_id?: Id, depth?: int = 1 }` | `{ revision, title, path, pages: [{id, name, children: [NodeSummary]}], tokens, styles: [{id, name, kind}], components: [{id, name}] }`. With no `page_id`, every page with top-level frames only. |
| `get_node` | `{ id: Id, depth?: int = 1, include_geometry?: bool = true }` | `{ node: NodeJson, geometry?: { x, y, width, height, rotation } /* absolute, page coords */, parent_id, index, path: [names] }`. Children below `depth` are summarised. |
| `find_nodes` | `{ name?: string /* substring, case-insensitive */, kind?: Kind, within?: Id, text?: string, limit?: int = 50 }` | `{ results: [NodeSummary] }` |
| `get_selection` | `{}` | `{ page_id, ids: [Id], nodes: [NodeSummary], focused_frame_id?: Id }` — what the user has selected right now, plus the top-level frame it is in. |
| `get_tokens` | `{}` | full `tokens` and `styles` objects |

`NodeSummary = { id, name, kind, width, height, children?: int }`.

**Write**

| Tool | Arguments | Returns |
| ---- | --------- | ------- |
| `create_node` | `{ parent_id: Id /* node or page id */, index?: int, node: NodeSpec, select?: bool = false }` | `{ revision, id, ids: {path: Id} }` — `id` of the root; `ids` maps each created node's name path to its ID. |
| `update_node` | `{ id: Id, patch: Patch, if_revision?: int }` | `{ revision, id }` |
| `update_nodes` | `{ updates: [{ id: Id, patch: Patch }], if_revision?: int }` | `{ revision, ids }` — one transaction. |
| `delete_nodes` | `{ ids: [Id], if_revision?: int }` | `{ revision, deleted: [Id] }` |
| `move_node` | `{ id: Id, parent_id?: Id, index?: int, x?: number, y?: number }` | `{ revision, id }` — reparent and/or reorder and/or reposition. |
| `duplicate_node` | `{ id: Id, parent_id?: Id, index?: int, offset?: [x, y] }` | `{ revision, id }` |
| `set_tokens` | `{ colors?: {name: color \| null}, spacing?: {name: number \| null}, radii?: …, fonts?: … }` | `{ revision }` — `null` removes. |
| `upsert_style` | `{ id?: Id, name: string, kind: "fill"\|"stroke"\|"text"\|"effect", value: StyleJson }` | `{ revision, id }` |
| `apply_style` | `{ node_ids: [Id], style_id: Id \| null, slot?: "fill"\|"stroke"\|"text"\|"effect" /* default from style kind */ }` | `{ revision, ids }` — `null` detaches. |
| `create_component` | `{ from_id: Id, name?: string }` | `{ revision, component_id, instance_id }` |
| `create_instance` | `{ component_id: Id, parent_id: Id, index?: int, x?: number, y?: number, overrides?: { [componentNodeId]: Override } }` | `{ revision, id }` |
| `set_selection` | `{ ids: [Id], zoom_to?: bool = false }` | `{ ids }` — not a document change, not in history. Lets the agent point at things. |

**Look and export**

| Tool | Arguments | Returns |
| ---- | --------- | ------- |
| `screenshot` | `{ id: Id, scale?: number = 1, max_size?: int = 1600 /* longest side, px */, background?: color }` | MCP `image` content (`image/png`, base64) plus text `{ width, height, scale }`. Renders exactly what the canvas shows for that node, without selection chrome. |
| `export_code` | `{ id: Id, format: "html" \| "tailwind" \| "react", options?: { tokens?: "inline" \| "css_vars" = "css_vars", include_components?: bool = true, responsive?: bool = false } }` | `{ files: [{ path, language, content }] }` — e.g. `index.html` + `styles.css`, or `ButtonPrimary.tsx` + `Page.tsx` + `tailwind.theme.js`. |
| `export_file` | `{ id: Id, format: "png" \| "svg", path: string, scale?: number = 1 }` | `{ path, bytes }` — writes to disk. Path must end in `.png`/`.svg` to match `format` (any case) and be inside the document's directory or `$HOME`, with no hidden folder or file (name starting with `.`) below it. A symlink at the file is never followed or replaced; an existing file is replaced only if it is a regular file, through a temp file and rename. |

Not exposed as tools in v1: `undo`/`redo` (the user owns the undo stack; an
agent that wants to revert issues the inverse edit), saving the file (the user
saves; `get_document` reports `dirty`), opening files.

### 6.4 Prompts

One MCP prompt, `design_conventions`, returns a short text the agent can load
explaining the model: frames vs groups, auto-layout sizing modes, token
reference syntax, "screenshot after edits to check your work", and that edits
are undoable by the user.

### 6.5 Resources and selection awareness

- Resource `design://selection` (JSON, same shape as `get_selection`).
- Resource `design://document` (JSON summary, same as `get_document`).
- Both support `resources/subscribe`; the app sends
  `notifications/resources/updated` when the selection changes (debounced
  250 ms) or a transaction commits.

Client support for resource subscriptions varies, so the guaranteed path is
the `get_selection` tool: the agent asks "what am I looking at" when the user
says "this" or "the selected button". The `design_conventions` prompt tells
agents to do so.

### 6.6 MCP crate

Use `rmcp` (the official Rust MCP SDK) for protocol handling, with its
transport over the unix stream (any `AsyncRead + AsyncWrite`). If `rmcp`'s
version or API is a poor fit, the fallback is a hand-written JSON-RPC layer on
`serde_json` + `tokio` (initialize, tools/list, tools/call, resources/*,
prompts/*); it is a few hundred lines and needs no new crates. The `agent`
crate hides the choice behind its own types.

Decided: `rmcp` 3.5.1 (features `server`, `transport-io`) with `schemars`
1.2.2; the fallback was not needed. v1 has 20 tools: 5 read, 12 write
(`set_selection` included, though it is not a document change), `screenshot`,
`export_code` and `export_file`.

### 6.7 Headless: no window

An agent on a computer where nobody has the app open (Jackie's Designer, a
server, a test) still makes and edits designs. The `headless` crate gives the
same tools two ways in, with no window and no display server:

- **`tessel render FILE`** renders one node (default: the first layer of the
  first page, which in a design file is its frame) to PNG, JPEG or SVG, by
  `-o`'s extension or `--format`, at `--scale`. JPEG has no alpha, so it is
  drawn on `--background` or white. `--layers OUT.json` also writes the
  **layer map**: the exported node and every visible layer under it, back to
  front, each with `id`, `name`, `kind`, `parent`, its axis-aligned box in the
  image's pixels and, where it has them, `text` and `image` (the asset shown).
  The topmost layer under a point is the last one listed whose box holds it;
  a phone uses that to edit what was tapped.
- **`tessel mcp --headless --root DIR [--images DIR]... [--templates DIR]
  [--fonts DIR]... [--open FILE]`** serves
  the §6.3 tools over stdio on the design files in `DIR`, plus four document
  tools only this mode lists: `list_documents`, `open_document`,
  `new_document` (one page, one frame named "Design" of the given size, or a
  copy of a template and its images; it makes plain subfolders) and `import_image` (a PNG, JPEG, GIF or WebP of at
  most 40 MB, kept in memory until a layer uses it). One document is open at
  a time. Each committed transaction is saved to its file at once (assets
  first, then the document, atomically), so there is no save tool and no
  dirty state. Opening another document bumps the revision, so an
  `if_revision` from the last one fails. `set_selection` is remembered but
  shows nothing.

**Templates** are the `.tessel.json` files directly in `--templates DIR`, read
only. `list_documents` lists each by `name` (the file name without
`.tessel.json`) with its title, size, and `meta.description`, a sentence for
the agent choosing one, which the copy drops. A template whose colours and
fonts are tokens (`{color.brand}`, `{font.heading}`) takes a brand with one
`set_tokens`. **`--fonts DIR`** (also on `tessel render`) loads the `.ttf`,
`.otf` and `.ttc` files in `DIR` as if installed, for this process only;
`list_documents` lists their families. A family that is neither installed nor
added still falls back to sans-serif.

Every path is checked against `DIR`: relative paths are relative to it,
absolute ones must be inside it, and nothing is read or written through `..`,
a hidden folder, or a symlink. `import_image` may also read from each
`--images DIR`. `export_file` writes only inside `DIR`, narrower than the app's
document-folder-or-`$HOME` rule.

Rendering opens a wgpu device with no surface: the hardware adapter if one
works, otherwise the fallback adapter, which on Linux is Mesa's lavapipe
(software Vulkan; `vulkan-swrast` on Arch, `mesa-vulkan-drivers` on Debian
and Ubuntu). `TESSEL_GPU=software` skips the hardware adapter. With neither,
PNG and JPEG fail with that advice; SVG needs no GPU.

---

## 7. Rendering and performance

### 7.1 Frame structure

Each egui frame:

1. Drain agent requests; apply commands (§5.4).
2. If the document changed, run incremental layout (§7.3).
3. egui lays out panels: toolbar (top), layers + pages (left), inspector
   (right), status bar (bottom), canvas (center, `egui::CentralPanel`).
4. The canvas widget handles input (tools, pan/zoom, hit-testing) and, if
   anything changed, re-renders the vello scene into a wgpu texture sized to
   the canvas rect × `pixels_per_point`.
5. The texture is shown with an `egui::Image` from a texture registered via
   `egui_wgpu::Renderer::register_native_texture` (re-registered only on
   resize).
6. Selection chrome (bounding boxes, handles, hover outlines, snapping guides,
   layout padding/gap overlays, marquee) is drawn with the egui `Painter`
   on top of the canvas image. It is cheap, crisp at any zoom, and keeps
   vello's scene pure document content (the same scene as screenshots).

### 7.2 Sharing the GPU

eframe's wgpu backend exposes its `wgpu::Device` and `Queue`
(`frame.wgpu_render_state()`). `render` creates one `vello::Renderer` on that
device, so canvas, screenshots and egui share one device and no pixels are
copied through the CPU. **This requires vello to use the same wgpu major
version as eframe 0.36 (wgpu 30).** Verified: vello 0.11.0 (2026-10-02)
depends on wgpu ^30, peniko ^0.6.1 and skrifa ^0.44, matching eframe 0.36, so
eframe stays on 0.36 and no pin or patch is needed.

vello renders with compute shaders; on Linux that needs Vulkan (Mesa
RADV/ANV, or NVIDIA proprietary). The app requests `Backends::VULKAN` first,
falling back to GL with a warning that the canvas may be slow or unsupported.

### 7.3 Layout pipeline

- `layout` keeps a persistent taffy tree mirroring frames with auto-layout and
  their children, keyed by `NodeId`. On change it updates only affected taffy
  nodes' styles and marks them dirty; taffy re-computes only dirty subtrees.
- Output: `Geometry`, a `HashMap<NodeId, NodeGeometry>` with each node's
  absolute transform and bounds in page coordinates. Consumed by `render`,
  hit-testing, the inspector, MCP reads and export.
- Text: a parley `FontContext`/`LayoutContext` lives in `layout`; text
  layouts are cached by (content, style, width constraint) and reused by
  `render` for glyph runs, so text is shaped once per change, not per frame.
- Fonts: system fonts via fontique (fontconfig on Linux). Missing font →
  fallback to the default sans with a warning badge on the node and in
  `get_node`.

### 7.4 Scene building and caching

- Scenes are built per **top-level frame** (and per loose top-level node).
  Each has a cached `vello::Scene` in frame-local coordinates, invalidated
  when any node in it changes (§5.5).
- Each canvas redraw builds a root scene by `append`ing the cached scenes with
  the view transform (pan/zoom) applied, skipping those outside the viewport.
  Pan and zoom therefore cost an append per visible frame, not a re-encode.
- Shapes are encoded with kurbo (`RoundedRect`, `Ellipse`, `BezPath` from SVG
  path data); paints map to peniko brushes; shadows use vello's blurred
  rounded rect (`draw_blurred_rounded_rect`) for rect/frame and an
  approximation for other shapes; clipping frames use `push_layer`.
- Images: decoded off the UI thread (`image` crate) into a cache keyed by
  asset hash, uploaded as peniko images; downscaled mip copies for zoomed-out
  views of large photos.
- The canvas only redraws on change: input, document change, animation (none
  in v1), or agent edits. Idle CPU/GPU use is ~0.

### 7.5 Hit-testing

CPU-side, from `Geometry`: walk visible top-level frames in front-to-back
order, test bounds then exact shape (kurbo `contains` for paths/ellipses,
stroke-aware with a few screen pixels of tolerance). Results respect
`locked`/`visible` and the "enter group" rule (click selects the outermost
unselected group; double-click enters). No GPU picking.

### 7.6 Offscreen rendering (screenshots, PNG export)

`render::render_node(doc, geometry, id, scale) -> RgbaImage`: build the scene
for that node alone, render to an offscreen texture of the node's bounds ×
scale, copy to a buffer, read back. Runs on the UI thread's device between
frames (requests come through the same channel as commands). Large renders
are tiled at 4096 px to stay within texture limits. `export` encodes PNG with
the `png` crate.

### 7.7 Performance targets and budgets

| Scenario | Target |
| -------- | ------ |
| Pan/zoom, 2,000 nodes visible, 4K display | ≤ 8 ms per frame (120 Hz) on a mid-range iGPU |
| Drag one node inside a 500-node frame | ≤ 8 ms (re-encode one frame scene + layout of its subtree) |
| Open a 5,000-node file | ≤ 500 ms to first paint |
| Agent `update_node` round trip (socket → applied → reply) | ≤ 1 frame + 5 ms |
| `screenshot` of a 1440×900 frame at 1× | ≤ 100 ms |
| Idle | no redraws, < 1% CPU |

Measured with a `--bench-scene N` flag that generates a synthetic document and
logs frame times (`tracing` spans around layout, scene build, render).

### 7.8 Hyprland / Wayland notes

- winit Wayland backend; set `app_id` to `tessel` via
  `ViewportBuilder::with_app_id`.
- Respect fractional scale (`wp_fractional_scale_v1`); the canvas texture is
  sized in physical pixels so 1.25×/1.5× scales are crisp.
- Hyprland tiles new windows. The app uses a single window: color pickers,
  menus and dialogs are in-window egui popups, so nothing needs float rules.
  File dialogs use `rfd` through the XDG desktop portal.
- Trackpad: two-finger scroll pans, pinch zooms (winit `PinchGesture`),
  Ctrl+scroll zooms around the cursor, Space+drag and middle-drag pan.
- Clipboard: egui/winit clipboard for text; copy/paste of nodes uses a JSON
  payload (same as the file format) with a `text/plain` fallback so nodes can
  be pasted into an agent chat and back.

---

## 8. Export

All exports are deterministic, so their output can be golden-tested.

- **PNG**: §7.6. Scale 1×/2×/3×.
- **SVG**: hand-written writer in `export` (no crate). Frames → `<g>` with
  clip paths, rect → `<rect rx>`, ellipse → `<ellipse>`, vector → `<path>`,
  text → `<text>` with font attributes (glyphs not outlined in v1), gradients
  → `<defs>`, images → `<image href>` with a data URI.
- **HTML + CSS**: one element per frame/node, class per node named from the
  layer name (`.pricing-card`), CSS in a separate file. Auto-layout → flexbox
  per §3.5's table; absolute children → `position: absolute` inside a
  `position: relative` parent; tokens → `:root` CSS custom properties and
  `var(--color-primary)` references; text styles → classes.
- **HTML + Tailwind**: same structure, utilities instead of CSS; tokens →
  a `tailwind` theme extension (v4 `@theme` CSS block by default, v3
  `tailwind.config.js` option); arbitrary values (`w-[372px]`) only when a
  value is not a token.
- **React**: TSX + Tailwind. Each component → a function component file named
  from the component name; `ComponentProp`s → typed props (`label: string`,
  `showIcon?: boolean`); instances → `<ButtonPrimary label="Buy" />`; a
  top-level frame → a page component.
- **Strokes** in code export: an `inside` stroke on a leaf becomes a CSS
  `border` (boxes are `border-box`, so the size is unchanged). A frame with
  children, and any `center` or `outside` stroke, becomes an `outline` with an
  `outline-offset`, because a border would move the children while the design's
  strokes never take space. Only the topmost stroke paint is used.
- Layer names drive class/component names; semantic hints (`semantic`) pick
  tags. The exporter never invents behavior (no handlers, no state).

The point of exporting is to give an agent a faithful starting point plus the
screenshot. Exports note at the top which design file, frame ID and revision
they came from.

---

## 9. App UI (v1 surface)

- **Toolbar**: Move (V), Frame (F), Rectangle (R), Ellipse (O), Text (T),
  Image (shift+Ctrl+K), Hand (H / Space). Zoom menu (fit, 100%, selection).
- **Left panel**: pages list; layers tree (rename, reorder by drag, reparent,
  visibility/lock toggles, multi-select, collapse); components list.
- **Right panel (inspector)**: position/size/rotation; auto-layout section;
  layout child sizing; fills/strokes/shadows with style pickers; text
  properties; token pickers on every color/spacing field; component overrides
  for instances; export section.
- **Canvas**: select, marquee, move, resize with handles (Shift = keep aspect,
  Alt = from center), rotate (outside corner handles), snapping to edges/
  centers of siblings and pixel grid, smart distance labels, double-click to
  edit text inline (egui `TextEdit` overlaid at the node's position and scale).
- **Shortcuts**: the common design-tool set where it does not conflict with Hyprland
  defaults (Super-based bindings are left to the compositor).
- **Status bar**: zoom, cursor position, revision, agent connections.

---

## 10. Milestones

Each milestone ends with a working app and tests. The tables list the crates
a milestone adds; the versions are those in `Cargo.lock`.

### M1 — canvas basics

_Status: Done: canvas, layout, render and app crates; commands and undo from the start._

Scope: window runs on Hyprland (native Wayland); pan/zoom canvas; draw rect,
ellipse and text (single style, inline editing); select, marquee, move, resize;
layers panel (tree, rename, reorder, visibility); new/open/save/save-as of
`.tessel.json`; one page. All edits already go through `Command`s (no
shortcuts around the model), and basic undo/redo (Ctrl+Z / Ctrl+Shift+Z) is
wired since it is nearly free once commands exist.

Crates: `doc`, `layout` (geometry only, no taffy yet: absolute positioning
plus parley text measurement), `render`, `app`.

| Crate |
| ----- |
| eframe, egui, egui-wgpu, wgpu, winit |
| serde, serde_json |
| uuid (ID entropy) |
| rfd (open/save dialogs via portal) |
| anyhow, thiserror, tracing, pollster |
| vello 0.11.0 (depends on wgpu 30, see §7.2) |
| parley 0.12.0, fontique 0.12.0 |
| kurbo 0.13.1, peniko 0.6.1, skrifa 0.44.0 |

Exit criteria: draw, select, move, resize, rename and reorder in the layers
panel, save, quit, reopen and see the same document; the saved file is
canonical (save → load → save is byte-identical, tested); runs under
`WAYLAND_DISPLAY` with no XWayland; command inverse property tests pass.

### M2 — styling, structure, inspector

_Status: Done: images and assets, inspector, pages and structure, snapping and guides, history, autosave, reload, SVG import._

Scope: inspector panel; fills (solid, linear/radial gradient), strokes, corner
radius, opacity, drop shadows; frames (with clipping) and groups; images
(insert from file, paste, fit modes, assets dir); vector nodes (import SVG path
data, basic point editing optional); multiple pages; z-order commands;
copy/paste/duplicate; snapping and smart guides; full history (coalescing,
labels, activity list); autosave/recovery; file reload on external change.

| Crate |
| ----- |
| image, png |
| everything from M1 |

No new crates expected.

### M3 — auto-layout and design tokens

_Status: Done: tokens and styles, taffy auto-layout, their UIs, radius tokens. Browser parity is the opt-in `scripts/layout-parity.sh`._

Scope: auto-layout on frames (direction, gap, padding, align, justify, wrap);
child sizing fixed/hug/fill, absolute children, min/max; constraints in
non-auto-layout frames; on-canvas padding/gap handles; tokens (colors,
spacing, radii, fonts) with an editor panel; fill/stroke/text/effect styles;
token and style pickers in the inspector.

| Crate |
| ----- |
| taffy 0.15.0 |

Exit criteria: a test suite of layout fixtures (doc → geometry) whose expected
results were checked against the same layout rendered by a browser as CSS
(fixtures committed as JSON; the browser check is a manual/CI step, see §11).

### M4 — agent (MCP)

_Status: Done: export crate (PNG, SVG), agent crate on rmcp 3.5.1, socket, stdio bridge and agent indicator._

Scope: `agent` crate; unix socket listener; `tessel mcp` stdio bridge
with `--launch`; all read and write tools in §6.3 plus `screenshot` and
`export_file` (PNG/SVG); `design_conventions` prompt; selection resources and
subscriptions; agent origin labels in history and activity; connection
indicator and read-only toggle; offscreen rendering.

| Crate |
| ----- |
| tokio |
| schemars (the version rmcp uses) |
| **rmcp** 3.5.1 (server + transport-io features) |

Exit criteria: Claude Code, registered with `claude mcp add`, can read a
document, build a screen from a text prompt via `create_node` with nested
children, screenshot it, and the user can undo the whole thing in one Ctrl+Z.
Integration tests drive the server over a socket against a headless
`AppHandle` fake.

### M5 — components and code export

_Status: Done: components model, HTML/Tailwind/React and token export, Export window, components UI with master editing._

Scope: components (create from selection, edit master, component props),
instances with overrides, detach, instance swap; `export_code` in html,
tailwind and react formats; token export (CSS vars, Tailwind theme); export
panel in the UI (copy to clipboard / save files); `create_component` and
`create_instance` tools.

| Crate |
| ----- |
| none new |

Exit criteria: golden tests for each export format on fixture documents;
exported HTML of the layout fixtures renders in a browser matching the layout
engine's geometry (the opt-in `scripts/code-parity.sh`, §11).

### After v1 (candidates, not committed)

Rich text spans, boolean ops, a pen tool, prototyping links, variables/modes
(light/dark token sets), HTTP MCP transport, import from other design tools, plugins,
multiplayer.

---

## 11. Testing strategy

- **doc**: unit tests per command; property tests for apply/inverse and
  canonical round-trip (v1 uses a small seeded random generator on
  `uuid`/std instead of `proptest`); golden files for the canonical
  writer; migration tests per format version.
- **layout**: fixture documents → expected geometry JSON. The fixtures are
  also exported to HTML so they can be checked against a browser for CSS
  parity. Browser checks are opt-in scripts, never part of plain
  `cargo test`: `scripts/layout-parity.sh` writes the HTML twins of the layout
  fixtures and runs the fixture tests (open the pages in a browser for the
  comparison), and `scripts/code-parity.sh` (`CODE_PARITY=1`) exports the
  fixtures as HTML + CSS, loads each in headless Chromium and compares every
  box with the layout engine's geometry (0.5 px, 3 px for text); once opted
  in it fails without Chromium. A headless browser is not assumed to be
  available: the checks are opt-in.
- **render**: golden PNGs where a GPU adapter is available; tests skip with a
  message when no adapter is found (CI may lack Vulkan).
- **export**: golden text outputs; the opt-in code parity check above.
- **agent**: protocol tests over an in-memory duplex stream and over a real
  unix socket, against a fake `AppHandle`; schema snapshot test so tool
  schemas change only on purpose.
- **app**: thin; manual test checklist per milestone on Hyprland.

## 12. Open questions

1. ~~vello release vs wgpu 30 (§7.2)~~ Closed: vello 0.11.0 uses wgpu 30, so
   eframe stays on 0.36.
2. ~~Tailwind v4 (`@theme`) as default export vs v3 config~~ Closed: v4 is the default; v3 is a choice in the Export window (and `export_code`'s Tailwind option).
3. Should agents be allowed `undo` of their own last transaction? Off for v1.
4. Rich text spans: needed early for real web designs (bold word in a
   sentence)? Currently after v1.
5. Do we want a project-level tokens file shared by several design files?
   Currently tokens are per document.
