//! Computed geometry for a tessel document (§7.3): each node's
//! absolute transform and bounds in page coordinates, text measured and
//! shaped with parley, and CPU hit-testing (§7.5).
//!
//! Frames with auto-layout place their flow children with taffy (§3.5,
//! [`flex`]); everything else sits at its stored `x`/`y` inside its parent.
//! Text nodes with automatic sizing take their size from parley.

pub mod constraints;
pub mod flex;
pub mod geometry;
pub mod hit;
pub mod parity;
pub mod text;
pub mod writeback;

pub use constraints::apply_constraints;
pub use flex::{effective_sizing, resolve_gaps, resolve_space};
pub use geometry::{Geometry, LayoutEngine, NodeGeometry, TextGeometry};
pub use hit::{ancestors, hit_test, marquee_select, HitOptions};
pub use text::{apply_transform, effective_text_style, resolve_family, TextLayout, TextMeasurer};
pub use writeback::{size_writeback, SizeWrite};

pub use kurbo;
pub use parley;
