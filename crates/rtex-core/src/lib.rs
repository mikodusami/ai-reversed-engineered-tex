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
//! | `JobProcessing` (doTypeset/doBibtex/...) | [`build`]            |
//! | `TSLogWindow`, texloganalyser            | [`log`]              |
//! | `SyncTeX` (+ external `synctex` tool)    | [`synctex`]          |
//! | Tags/Labels menus (`doTag:`, `doLabel:`) | [`outline`]          |
//! | `SyntaxHighlighting`                     | [`highlight`]        |
//! | Trash AUX Files                          | [`clean`]            |
//! | Command Completion (`CommandCompletion.txt`) | [`completion`]   |
//! | autocompletion.plist pairs, Esc `\end` | [`autopair`]         |
//! | SaveRelated (`checkFileLinks:`)          | [`project`]          |

pub mod autopair;
pub mod build;
pub mod clean;
pub mod completion;
pub mod directives;
pub mod engine;
pub mod ffi;
pub mod highlight;
pub mod log;
pub mod outline;
pub mod project;
pub mod root;
pub mod synctex;

pub use build::{build, BuildOptions, BuildReport};
pub use directives::Directives;
