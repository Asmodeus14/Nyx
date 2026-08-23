//! `nyx-web` — the HTML/CSS engine.
//!
//! Pipeline, in the order a page moves through it:
//! `dom` (html5ever → arena) → `js` (scripts mutate the tree) → `css` (cssparser → stylesheet) →
//! `style` (match + cascade) → layout → paint.
//!
//! ★ Scripts run BEFORE the cascade, not after: a script that changes text or attributes changes
//! what the selectors match, so cascading first would style the pre-script document and then paint
//! the post-script one.
//!
//! Portable `std`, no OS coupling, so the whole engine is host-testable with `cargo test` and only
//! the app around it needs Nyx. Given the alternative is debugging a layout bug through a serial
//! log on a laptop, that property is worth protecting.

pub mod css;
pub mod dom;
pub mod js;
pub mod layout;
pub mod style;

pub use css::Stylesheet;
pub use dom::{Dom, Node, NodeId, NodeKind};
pub use js::{inline_scripts, JsOutcome};
pub use layout::{layout, DisplayItem, FontMetrics, ImageSource, LayoutResult, Link, NoImages};
pub use style::{ComputedStyle, Display, StyleTree};
