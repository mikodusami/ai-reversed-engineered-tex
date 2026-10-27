//! rtex-core: the engine room of RTeX, a TeXShop-style LaTeX workbench.
//!
//! Module map against TeXShop 5.57 (classes recovered from the binary with
//! REA; see docs/RE-NOTES.md):
//!
//! | TeXShop                                  | rtex-core            |
//! |------------------------------------------|----------------------|
//! | `% !TEX` parsing in TSDocument           | [`directives`]       |

pub mod directives;

pub use directives::Directives;
