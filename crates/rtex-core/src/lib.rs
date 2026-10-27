//! rtex-core: the engine room of RTeX, a TeXShop-style LaTeX workbench.
//!
//! Module map against TeXShop 5.57 (classes recovered from the binary with
//! REA; see docs/RE-NOTES.md):
//!
//! | TeXShop                                  | rtex-core            |
//! |------------------------------------------|----------------------|
//! | `% !TEX` parsing in TSDocument           | [`directives`]       |
//! | `RootFile`, `checkRootFile:forTask:`     | [`root`]             |
//! | Typeset menu + `Engines/*.engine`        | [`engine`]           |
//! | `TSLogWindow`, texloganalyser            | [`log`]              |
//! | Tags/Labels menus (`doTag:`, `doLabel:`) | [`outline`]          |
//! | `SyntaxHighlighting`                     | [`highlight`]        |

pub mod directives;
pub mod engine;
pub mod highlight;
pub mod log;
pub mod outline;
pub mod root;

pub use directives::Directives;
