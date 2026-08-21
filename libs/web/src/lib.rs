//! `nyx-web` — the HTML/CSS engine.
//!
//! Pipeline, in the order a page moves through it:
//! `dom` (html5ever → arena) → `css` (cssparser → stylesheet) → `style` (match + cascade) →
//! layout → paint.
//!
//! Portable `std`, no OS coupling, so the whole engine is host-testable with `cargo test` and only
//! the app around it needs Nyx. Given the alternative is debugging a layout bug through a serial
//! log on a laptop, that property is worth protecting.

pub mod css;
pub mod dom;
pub mod layout;
pub mod style;

pub use css::Stylesheet;
pub use dom::{Dom, Node, NodeId, NodeKind};
pub use layout::{layout, DisplayItem, FontMetrics, ImageSource, LayoutResult, Link, NoImages};
pub use style::{ComputedStyle, Display, StyleTree};
