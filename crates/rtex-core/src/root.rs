//! Root-file resolution (TeXShop: `-[TSDocument checkRootFile:forTask:]`).
//!
//! A file containing `% !TEX root = other.tex` is typeset by typesetting the
//! root instead. Roots can chain; we follow them with a cycle guard.

use crate::directives::Directives;
use std::io;
use std::path::{Component, Path, PathBuf};

const MAX_CHAIN: usize = 16;

/// Follow `% !TEX root` directives starting at `file`. Returns the root path
/// (absolute, normalised) and its directives.
pub fn resolve_root(file: &Path) -> io::Result<(PathBuf, Directives)> {
    let mut current = absolutize(file)?;
    let mut seen = Vec::new();
    loop {
        let text = read_lossy(&current)?;
        let d = Directives::parse(&text);
        let Some(root) = d.root.clone() else { return Ok((current, d)) };
        if seen.contains(&current) || seen.len() >= MAX_CHAIN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("% !TEX root cycle at {}", current.display()),
            ));
        }
        seen.push(current.clone());
        let base = current.parent().unwrap_or(Path::new("/"));
        let next = normalize(&base.join(expand_tilde(&root)));
        if next == current {
            return Ok((current, d));
        }
        if !next.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("root file {} (named in {}) does not exist", next.display(), current.display()),
            ));
        }
        current = next;
    }
}

/// Read a file as UTF-8, replacing invalid bytes (legacy Latin-1 sources).
pub fn read_lossy(p: &Path) -> io::Result<String> {
    let bytes = std::fs::read(p)?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

pub fn absolutize(p: &Path) -> io::Result<PathBuf> {
    let p = expand_tilde(&p.to_string_lossy());
    Ok(normalize(&if p.is_absolute() { p } else { std::env::current_dir()?.join(p) }))
}

fn expand_tilde(s: &str) -> PathBuf {
    match (s.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(s),
    }
}

/// Lexically normalise `.` and `..` (does not touch the filesystem, so it
/// also works for SyncTeX paths like `/a/b/./c.tex`).
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize(Path::new("/a/b/./c/../d.tex")), PathBuf::from("/a/b/d.tex"));
    }

    #[test]
    fn follows_root_chain() {
        let dir = std::env::temp_dir().join(format!("rtex-root-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("main.tex"), "% !TEX TS-program = xelatex\n\\documentclass{article}").unwrap();
        std::fs::write(dir.join("sub/mid.tex"), "% !TEX root = ../main.tex\n").unwrap();
        std::fs::write(dir.join("sub/leaf.tex"), "% !TEX root = mid.tex\n").unwrap();
        let (root, d) = resolve_root(&dir.join("sub/leaf.tex")).unwrap();
        assert_eq!(root, normalize(&dir.join("main.tex")));
        assert_eq!(d.program.as_deref(), Some("xelatex"));
        std::fs::write(dir.join("a.tex"), "% !TEX root = b.tex\n").unwrap();
        std::fs::write(dir.join("b.tex"), "% !TEX root = a.tex\n").unwrap();
        assert!(resolve_root(&dir.join("a.tex")).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
