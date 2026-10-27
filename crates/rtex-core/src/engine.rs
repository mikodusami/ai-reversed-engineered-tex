//! Engine registry (TeXShop: the Typeset menu's TeX/LaTeX/ConTeXt commands
//! plus `~/Library/TeXShop/Engines/*.engine` scripts; see `defaultEngineCall:`,
//! `absoluteEnginePath:` and the `.engine` files bundled in TeXShop.app).
//!
//! Built-in names run a TeX binary directly through rtex's in-process
//! pipeline. The TeXShop `*mk` engines (pdflatexmk, xelatexmk, ...) wrap
//! latexmk in TeXShop; rtex maps them onto its own pipeline, which makes
//! the same rerun/BibTeX/makeindex decisions without starting Perl.
//! Any other name is looked up as a `.engine` script, so existing TeXShop
//! engines keep working.

use serde::Serialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_BIN_DIR: &str = "/Library/TeX/texbin";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Engine {
    /// Run a TeX binary directly.
    Tex {
        program: String,
        /// LaTeX format (so BibTeX/makeindex/reruns apply), as opposed to plain TeX.
        latex: bool,
        shell_escape: bool,
    },
    /// ConTeXt (`context` handles its own reruns).
    Context,
    /// A TeXShop-compatible `.engine` script: `script <file name> <parameter>`.
    Script { name: String, path: PathBuf },
}

impl Engine {
    pub fn program(&self) -> &str {
        match self {
            Engine::Tex { program, .. } => program,
            Engine::Context => "context",
            Engine::Script { name, .. } => name,
        }
    }
}

/// Directories searched for `.engine` scripts, in priority order.
pub fn engine_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        v.push(home.join("Library/RTeX/Engines"));
        v.push(home.join("Library/TeXShop/Engines"));
    }
    v
}

/// Resolve an engine name (`% !TEX program` value or user choice).
pub fn resolve(name: &str) -> Result<Engine, String> {
    resolve_in(name, &engine_dirs())
}

pub fn resolve_in(name: &str, dirs: &[PathBuf]) -> Result<Engine, String> {
    let lower = name.trim().to_ascii_lowercase();
    let tex = |p: &str, latex: bool, se: bool| Engine::Tex { program: p.into(), latex, shell_escape: se };
    let builtin = match lower.as_str() {
        "pdflatex" | "latex" | "pdflatexmk" | "latexmk" => Some(tex("pdflatex", true, false)),
        "sepdflatexmk" => Some(tex("pdflatex", true, true)),
        "xelatex" | "xelatexmk" => Some(tex("xelatex", true, false)),
        "sexelatexmk" => Some(tex("xelatex", true, true)),
        "lualatex" | "lualatexmk" => Some(tex("lualatex", true, false)),
        "selualatexmk" => Some(tex("lualatex", true, true)),
        "pdftex" | "tex" => Some(tex("pdftex", false, false)),
        "xetex" => Some(tex("xetex", false, false)),
        "luatex" => Some(tex("luatex", false, false)),
        "context" | "contextmk" => Some(Engine::Context),
        _ => None,
    };
    if let Some(e) = builtin {
        return Ok(e);
    }
    for dir in dirs {
        let p = dir.join(format!("{name}.engine"));
        if p.is_file() {
            return Ok(Engine::Script { name: name.to_string(), path: p });
        }
    }
    Err(format!("unknown engine '{name}' (no built-in and no {name}.engine in {:?})", dirs))
}

#[derive(Debug, Serialize)]
pub struct EngineInfo {
    pub name: String,
    pub kind: &'static str,
    pub path: Option<PathBuf>,
}

/// Built-ins plus every `.engine` script found.
pub fn list() -> Vec<EngineInfo> {
    let mut out: Vec<EngineInfo> = ["pdflatex", "xelatex", "lualatex", "pdftex", "xetex", "luatex", "context"]
        .iter()
        .map(|n| EngineInfo { name: n.to_string(), kind: "builtin", path: None })
        .collect();
    for dir in engine_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut names: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "engine"))
            .collect();
        names.sort();
        for p in names {
            let name = p.file_stem().unwrap().to_string_lossy().to_string();
            if out.iter().any(|e| e.name == name) {
                continue;
            }
            let kind = if resolve_in(&name, &[]).is_ok() { "builtin-alias" } else { "script" };
            out.push(EngineInfo { name, kind, path: Some(p) });
        }
    }
    out
}

/// Locate a TeX binary: the configured bin dir first, then $PATH.
pub fn find_binary(bin_dir: &Path, program: &str) -> PathBuf {
    let p = bin_dir.join(program);
    if p.is_file() {
        return p;
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let c = dir.join(program);
            if c.is_file() {
                return c;
            }
        }
    }
    PathBuf::from(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_texshop_names() {
        assert_eq!(resolve_in("pdflatexmk", &[]).unwrap().program(), "pdflatex");
        assert_eq!(resolve_in("XeLaTeX", &[]).unwrap().program(), "xelatex");
        assert!(matches!(resolve_in("sepdflatexmk", &[]).unwrap(), Engine::Tex { shell_escape: true, .. }));
        assert!(matches!(resolve_in("tex", &[]).unwrap(), Engine::Tex { latex: false, .. }));
        assert!(resolve_in("nosuchengine", &[]).is_err());
    }

    #[test]
    fn finds_scripts() {
        let dir = std::env::temp_dir().join(format!("rtex-eng-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Custom.engine"), "#!/bin/sh\n").unwrap();
        let e = resolve_in("Custom", std::slice::from_ref(&dir)).unwrap();
        assert!(matches!(e, Engine::Script { .. }));
        std::fs::remove_dir_all(dir).ok();
    }
}
