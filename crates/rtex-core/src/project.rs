//! Files that belong to a document, for "save related files before
//! typesetting" (TeXShop: `-[TSDocument checkFileLinks:]` with the
//! `SaveRelated` preference, which saves open, edited documents named by the
//! root's `\input` lines; RTeX also follows `\include` and `\subfile`).

use crate::outline::strip_comment;
use crate::root::normalize;
use std::path::{Path, PathBuf};

const COMMANDS: &[&str] = &["\\input", "\\include", "\\subfile"];

/// Absolute paths of files `\input`/`\include`d by `text` (relative names
/// resolve against `dir`; a missing extension means `.tex`). Commented-out
/// lines are ignored.
pub fn included_files(text: &str, dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for line in text.lines() {
        let code = strip_comment(line);
        let mut rest = code;
        while let Some(i) = rest.find('\\') {
            rest = &rest[i..];
            let Some(cmd) = COMMANDS.iter().find(|c| {
                rest.starts_with(**c)
                    && !rest[c.len()..].starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '@')
            }) else {
                rest = &rest[1..];
                continue;
            };
            rest = &rest[cmd.len()..];
            let arg = argument(rest);
            if let Some(name) = arg.filter(|a| !a.is_empty() && !a.contains('\\') && !a.contains('#')) {
                let mut p = PathBuf::from(name);
                if p.extension().is_none() {
                    p.set_extension("tex");
                }
                let abs = normalize(&if p.is_absolute() { p } else { dir.join(p) });
                if !out.contains(&abs) {
                    out.push(abs);
                }
            }
        }
    }
    out
}

/// `{name}` or TeX-style ` name` after the command.
fn argument(s: &str) -> Option<&str> {
    let t = s.trim_start();
    if let Some(b) = t.strip_prefix('{') {
        return b.find('}').map(|e| b[..e].trim());
    }
    if t.len() == s.len() {
        return None; // `\inputfoo` is a different command
    }
    let end = t.find(|c: char| c.is_whitespace() || c == '}' || c == '%').unwrap_or(t.len());
    Some(&t[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_inputs_and_includes() {
        let text = "\\documentclass{book}\n\\input{preamble}\n\\include{chapters/one}\n\\input chapters/two.tex\n% \\input{commented}\n\\includegraphics{fig.png}\n\\subfile{appendix}\n\\input{preamble}\n";
        let got = included_files(text, Path::new("/doc"));
        let want: Vec<PathBuf> = ["/doc/preamble.tex", "/doc/chapters/one.tex", "/doc/chapters/two.tex", "/doc/appendix.tex"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn ignores_macros_as_arguments() {
        assert!(included_files("\\input{\\jobname.aux}\n\\def\\x#1{\\input{#1}}", Path::new("/d")).is_empty());
    }

    #[test]
    fn resolves_parent_dirs() {
        assert_eq!(included_files("\\input{../shared/defs}", Path::new("/a/b")), vec![PathBuf::from("/a/shared/defs.tex")]);
    }
}
