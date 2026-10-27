//! "Trash AUX Files" (TeXShop: `trashAUX`, `AggressiveTrashAUX`,
//! `OtherTrashExtensions` preference keys).

use std::path::{Path, PathBuf};

pub const AUX_EXTS: &[&str] = &[
    "aux", "log", "toc", "lof", "lot", "out", "bbl", "blg", "idx", "ind", "ilg", "nav", "snm", "vrb",
    "fls", "fdb_latexmk", "bcf", "run.xml", "glo", "gls", "glg", "nlo", "nls", "xdv", "dvi",
    "synctex.gz", "synctex", "loa", "thm", "brf", "mtc", "maf",
];

/// Remove auxiliary files of `root` (`stem.<ext>`); with `aggressive`, every
/// file with an auxiliary extension in the root's folder and below (TeXShop's
/// aggressive mode). Returns removed paths. The PDF is never removed.
pub fn clean(root: &Path, aggressive: bool, extra_exts: &[String]) -> std::io::Result<Vec<PathBuf>> {
    let dir = root.parent().unwrap_or(Path::new("."));
    let stem = root.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let exts: Vec<String> = AUX_EXTS.iter().map(|s| s.to_string()).chain(extra_exts.iter().cloned()).collect();
    let mut removed = Vec::new();
    if aggressive {
        walk(dir, &exts, &mut removed, 0)?;
    } else {
        // `\include` writes sub-aux files named in the main aux (`\@input{x.aux}`).
        if let Ok(aux) = std::fs::read_to_string(dir.join(format!("{stem}.aux"))) {
            for sub in aux.lines().filter_map(|l| l.strip_prefix("\\@input{")?.strip_suffix('}')) {
                let p = dir.join(sub);
                if sub.ends_with(".aux") && p.is_file() && std::fs::remove_file(&p).is_ok() {
                    removed.push(p);
                }
            }
        }
        for e in &exts {
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
