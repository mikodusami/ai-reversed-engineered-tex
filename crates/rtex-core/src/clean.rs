//! Build files: "Trash AUX Files" (TeXShop: `trashAUX`, `AggressiveTrashAUX`,
//! `OtherTrashExtensions` preference keys), plus RTeX's after-typeset
//! clean-up that leaves only the sources and the PDF in the folder.
//!
//! "Hide" mode moves build files into the per-document cache
//! (`~/Library/Caches/rtex/<doc>/stash/`) and moves them back before the
//! next typeset, so reruns stay incremental and SyncTeX keeps working.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const AUX_EXTS: &[&str] = &[
    "aux", "log", "toc", "lof", "lot", "out", "bbl", "blg", "idx", "ind", "ilg", "nav", "snm", "vrb",
    "fls", "fdb_latexmk", "bcf", "run.xml", "glo", "gls", "glg", "nlo", "nls", "xdv", "dvi",
    "synctex.gz", "synctex", "loa", "thm", "brf", "mtc", "maf",
];

/// What to do with build files once a typeset has finished.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AfterBuild {
    /// Leave them in the folder (TeXShop's behaviour).
    #[default]
    Keep,
    /// Move them to the cache; restored before the next typeset.
    Hide,
    /// Delete them (the SyncTeX file is kept in the cache so sync works).
    Delete,
}

impl AfterBuild {
    pub fn parse(s: &str) -> Option<AfterBuild> {
        match s.to_ascii_lowercase().as_str() {
            "keep" => Some(AfterBuild::Keep),
            "hide" => Some(AfterBuild::Hide),
            "delete" | "trash" => Some(AfterBuild::Delete),
            _ => None,
        }
    }
}

/// Build files of `root` that exist now: `stem.<aux ext>` next to it and the
/// `\include` sub-aux files listed in its `.aux`. Never the PDF or sources.
pub fn build_files(root: &Path) -> Vec<PathBuf> {
    let dir = root.parent().unwrap_or(Path::new("."));
    let stem = root.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let mut out = Vec::new();
    // `\include` writes sub-aux files named in the main aux (`\@input{x.aux}`).
    if let Ok(aux) = std::fs::read_to_string(dir.join(format!("{stem}.aux"))) {
        for sub in aux.lines().filter_map(|l| l.strip_prefix("\\@input{")?.strip_suffix('}')) {
            let p = dir.join(sub);
            if sub.ends_with(".aux") && !sub.contains("..") && p.is_file() {
                out.push(p);
            }
        }
    }
    for e in AUX_EXTS {
        let p = dir.join(format!("{stem}.{e}"));
        if p.is_file() {
            out.push(p);
        }
    }
    out
}

pub fn stash_dir(root: &Path) -> PathBuf {
    crate::build::doc_cache_dir(root).join("stash")
}

/// Apply an after-typeset mode. Returns the files moved out of the folder.
pub fn after_build(root: &Path, mode: AfterBuild) -> std::io::Result<Vec<PathBuf>> {
    if mode == AfterBuild::Keep {
        return Ok(Vec::new());
    }
    let dir = root.parent().unwrap_or(Path::new("."));
    let stash = stash_dir(root);
    if mode == AfterBuild::Delete {
        // Nothing but the SyncTeX file survives a delete.
        let _ = std::fs::remove_dir_all(&stash);
    }
    let mut moved = Vec::new();
    for f in build_files(root) {
        let name = f.to_string_lossy();
        let keep_copy = mode == AfterBuild::Hide || name.ends_with(".synctex.gz") || name.ends_with(".synctex");
        if keep_copy {
            let rel = f.strip_prefix(dir).unwrap_or(&f);
            let dest = stash.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            move_file(&f, &dest)?;
        } else {
            std::fs::remove_file(&f)?;
        }
        moved.push(f);
    }
    Ok(moved)
}

/// Before a typeset: put hidden build files back (never over newer files).
pub fn restore(root: &Path) -> std::io::Result<usize> {
    let dir = root.parent().unwrap_or(Path::new("."));
    let stash = stash_dir(root);
    let mut n = 0;
    let mut stack = vec![stash.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p.strip_prefix(&stash).unwrap_or(&p);
            let dest = dir.join(rel);
            if dest.exists() || !dest.parent().is_some_and(Path::is_dir) {
                let _ = std::fs::remove_file(&p);
                continue;
            }
            move_file(&p, &dest)?;
            n += 1;
        }
    }
    Ok(n)
}

/// The hidden copy of a file next to `root` (e.g. its SyncTeX file).
pub fn hidden(root: &Path, file_name: &str) -> Option<PathBuf> {
    let p = stash_dir(root).join(file_name);
    p.is_file().then_some(p)
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Different volumes: copy, then remove.
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// Remove auxiliary files of `root` (`stem.<ext>`); with `aggressive`, every
/// file with an auxiliary extension in the root's folder and below (TeXShop's
/// aggressive mode). Also drops hidden copies and the build cache. Returns
/// removed paths. The PDF is never removed.
pub fn clean(root: &Path, aggressive: bool, extra_exts: &[String]) -> std::io::Result<Vec<PathBuf>> {
    let dir = root.parent().unwrap_or(Path::new("."));
    let stem = root.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let exts: Vec<String> = AUX_EXTS.iter().map(|s| s.to_string()).chain(extra_exts.iter().cloned()).collect();
    let mut removed = Vec::new();
    if aggressive {
        walk(dir, &exts, &mut removed, 0)?;
    } else {
        for p in build_files(root) {
            if std::fs::remove_file(&p).is_ok() {
                removed.push(p);
            }
        }
        for e in extra_exts {
            let p = dir.join(format!("{stem}.{e}"));
            if p.is_file() && std::fs::remove_file(&p).is_ok() {
                removed.push(p);
            }
        }
    }
    let _ = std::fs::remove_dir_all(crate::build::doc_cache_dir(root));
    Ok(removed)
}

fn walk(dir: &Path, exts: &[String], removed: &mut Vec<PathBuf>, depth: u32) -> std::io::Result<()> {
    if depth > 4 {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)?.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, exts, removed, depth + 1)?;
        } else if exts.iter().any(|e| name.ends_with(&format!(".{e}"))) && std::fs::remove_file(&p).is_ok() {
            removed.push(p);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rtex-clean-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("chapters")).unwrap();
        let root = dir.join("main.tex");
        for (f, body) in [
            ("main.tex", "\\documentclass{book}"),
            ("main.pdf", "%PDF"),
            ("refs.bib", "@book{x}"),
            ("fig.png", "png"),
            ("main.aux", "\\relax\n\\@input{chapters/one.aux}\n"),
            ("chapters/one.aux", "\\relax"),
            ("chapters/one.tex", "text"),
            ("main.log", "log"),
            ("main.toc", "toc"),
            ("main.synctex.gz", "sync"),
        ] {
            std::fs::write(dir.join(f), body).unwrap();
        }
        (dir, root)
    }

    fn remaining(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = ["main.tex", "main.pdf", "refs.bib", "fig.png", "main.aux", "chapters/one.aux",
            "chapters/one.tex", "main.log", "main.toc", "main.synctex.gz"]
            .iter()
            .filter(|f| dir.join(f).exists())
            .map(|s| s.to_string())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn hide_then_restore_round_trip() {
        let (dir, root) = setup("hide");
        let cache = std::env::temp_dir().join(format!("rtex-cache-test-{}", std::process::id()));
        std::env::set_var("RTEX_CACHE_DIR", &cache);
        let moved = after_build(&root, AfterBuild::Hide).unwrap();
        assert_eq!(moved.len(), 5);
        assert_eq!(remaining(&dir), ["chapters/one.tex", "fig.png", "main.pdf", "main.tex", "refs.bib"]);
        assert!(hidden(&root, "main.synctex.gz").is_some());
        assert_eq!(restore(&root).unwrap(), 5);
        assert!(dir.join("chapters/one.aux").exists() && dir.join("main.aux").exists());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(crate::build::doc_cache_dir(&root)).ok();
    }

    #[test]
    fn delete_keeps_only_synctex_copy() {
        let (dir, root) = setup("delete");
        let cache = std::env::temp_dir().join(format!("rtex-cache-test-{}", std::process::id()));
        std::env::set_var("RTEX_CACHE_DIR", &cache);
        after_build(&root, AfterBuild::Delete).unwrap();
        assert_eq!(remaining(&dir), ["chapters/one.tex", "fig.png", "main.pdf", "main.tex", "refs.bib"]);
        assert!(hidden(&root, "main.synctex.gz").is_some());
        assert!(hidden(&root, "main.aux").is_none());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(crate::build::doc_cache_dir(&root)).ok();
    }

    #[test]
    fn keep_does_nothing() {
        let (dir, root) = setup("keep");
        assert!(after_build(&root, AfterBuild::Keep).unwrap().is_empty());
        assert_eq!(remaining(&dir).len(), 10);
        std::fs::remove_dir_all(&dir).ok();
    }
}
