//! Command completion, compatible with TeXShop's `CommandCompletion.txt`.
//!
//! Format (from TeXShop's "Command Completion" guide and the shipped file):
//! * one entry per line; `abbrev:=expansion` is an abbreviation (exact
//!   match), any other line is a completion (prefix match);
//! * `#INS#` marks the insertion point; two of them select the text between
//!   (`#INC#` is accepted as a synonym); `#RET#` is a newline that keeps the
//!   current indentation; `#T#` is a tab;
//! * `•` is a Mark (placeholder) and `•‹comment›` a Mark with a comment.
//!
//! Matching order: TeXShop walks the file from the bottom, so `{eq` offers
//! `{equation}`, then `{eqnarray}`, then `{eqnarray*}` (the file lists them in
//! the reverse order). We do the same.

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Abbreviation key (`\bsk` in `\bsk:=\bigskip`), or None for a completion.
    pub key: Option<String>,
    pub body: String,
}

#[derive(Debug, Default)]
pub struct Table {
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Expansion {
    pub text: String,
    /// Selection after insertion, UTF-16 units relative to the start of `text`.
    pub sel_start: u32,
    pub sel_len: u32,
}

/// Characters that end a word to the left of the cursor. `\` and `{` also
/// end it but belong to the word (TeXShop's "Word Boundary Characters").
pub fn is_boundary(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '.' | ',' | ';' | ':' | '{' | '}' | '(' | ')' | '\\')
}

impl Table {
    pub fn parse(text: &str) -> Table {
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with("%!TEX") || line.starts_with("% !TEX") {
                continue;
            }
            match line.split_once(":=") {
                Some((k, b)) if !k.is_empty() && !k.contains(char::is_whitespace) => {
                    entries.push(Entry { key: Some(k.to_string()), body: b.to_string() })
                }
                _ => entries.push(Entry { key: None, body: line.to_string() }),
            }
        }
        Table { entries }
    }

    /// Entries offered for `word`, best first (bottom of the file first).
    pub fn candidates(&self, word: &str) -> Vec<&Entry> {
        if word.is_empty() {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .rev()
            .filter(|e| match &e.key {
                Some(k) => k == word,
                None => e.body.starts_with(word) && e.body != word,
            })
            .filter(|e| seen.insert(e.body.as_str()))
            .collect()
    }
}

/// Turn an entry body into text plus the selection to make, honouring the
/// indentation of the line where completion happens.
pub fn expand(body: &str, indent: &str) -> Expansion {
    let body = body.replace("#INC#", "#INS#");
    let newline = format!("\n{indent}");
    let mut text = String::with_capacity(body.len());
    let mut marks: Vec<u32> = Vec::new();
    let mut u16_len = 0u32;
    let mut rest = body.as_str();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("#INS#") {
            marks.push(u16_len);
            rest = r;
        } else if let Some(r) = rest.strip_prefix("#RET#") {
            text.push_str(&newline);
            u16_len += newline.encode_utf16().count() as u32;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("#T#") {
            text.push('\t');
            u16_len += 1;
            rest = r;
        } else {
            let c = rest.chars().next().unwrap();
            text.push(c);
            u16_len += c.len_utf16() as u32;
            rest = &rest[c.len_utf8()..];
        }
    }
    let (sel_start, sel_len) = match marks.as_slice() {
        [] => (u16_len, 0),
        [a] => (*a, 0),
        [a, b, ..] => (*a.min(b), a.abs_diff(*b)),
    };
    Expansion { text, sel_start, sel_len }
}

/// Where completion files are looked for, in order.
pub fn search_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        v.push(home.join("Library/RTeX/CommandCompletion.txt"));
        v.push(home.join("Library/TeXShop/CommandCompletion/CommandCompletion.txt"));
    }
    v.push(PathBuf::from(
        "/Applications/TeX/TeXShop.app/Contents/Resources/TeXShop/CommandCompletion/CommandCompletion.txt",
    ));
    v
}

/// A small built-in table used when no completion file is installed.
const BUILTIN: &str = r"\usepackage{#INS#•#INS#}
\section{#INS#•#INS#}
\subsection{#INS#•#INS#}
\label{#INS#•#INS#}
\ref{#INS#•#INS#}
\cite{#INS#•#INS#}
\emph{#INS#•#INS#}
\textbf{#INS#•#INS#}
\frac{#INS#•‹num›#INS#}{•‹den›}
\includegraphics[width=#INS#•#INS#]{•}
\bfig:=\begin{figure}[#INS#htbp#INS#]#RET#\centering#RET#•#RET#\caption{•}#RET#\label{fig:•}#RET#\end{figure}•
\bitem:=\begin{itemize}#RET#\item #INS#•#INS##RET#\end{itemize}•
\benu:=\begin{enumerate}#RET#\item #INS#•#INS##RET#\end{enumerate}•
\beq:=\begin{equation}#RET##INS#•#INS##RET#\end{equation}•
{itemize}#RET#\item #INS#•#INS##RET#\end{itemize}•
{enumerate}#RET#\item #INS#•#INS##RET#\end{enumerate}•
{figure}#RET##INS#•#INS##RET#\end{figure}•
{equation}#RET##INS#•#INS##RET#\end{equation}•
\end{
\begin{
";

pub struct Loaded {
    pub source: String,
    pub table: Table,
}

type Cache = Mutex<Option<(PathBuf, Option<SystemTime>, Arc<Loaded>)>>;

/// The active table: the first existing file on `search_paths()`, reloaded
/// when it changes; the built-in table otherwise.
pub fn active() -> Arc<Loaded> {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let found = search_paths().into_iter().find(|p| p.is_file());
    let (path, mtime) = match &found {
        Some(p) => (p.clone(), std::fs::metadata(p).and_then(|m| m.modified()).ok()),
        None => (PathBuf::from("(built-in)"), None),
    };
    let mut guard = cache.lock().unwrap();
    if let Some((p, t, l)) = guard.as_ref() {
        if *p == path && *t == mtime {
            return l.clone();
        }
    }
    let text = found
        .as_deref()
        .and_then(|p| crate::root::read_lossy(p).ok())
        .unwrap_or_else(|| BUILTIN.to_string());
    let loaded = Arc::new(Loaded { source: path.display().to_string(), table: Table::parse(&text) });
    *guard = Some((path, mtime, loaded.clone()));
    loaded
}

/// Load a specific file (for tests and `rtex complete --file`).
pub fn load(path: &Path) -> std::io::Result<Loaded> {
    Ok(Loaded { source: path.display().to_string(), table: Table::parse(&crate::root::read_lossy(path)?) })
}

/// The word to the left of `cursor` (a byte index into `line`): from the
/// last boundary character, including a leading `\` or `{`.
pub fn word_before(line: &str, cursor: usize) -> &str {
    let head = &line[..cursor];
    let start = head.char_indices().rev().find(|(_, c)| is_boundary(*c)).map(|(i, c)| {
        if c == '\\' || c == '{' { i } else { i + c.len_utf8() }
    });
    &head[start.unwrap_or(0)..]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "%!TEX encoding = UTF-8 Unicode\n\\bsk:=\\bigskip\n\\bigskip#RET#\n{eqnarray*}#RET##INS#•#INS##RET#\\end{eqnarray*}•\n{eqnarray}#RET##INS#•#INS##RET#\\end{eqnarray}•\n{equation}#RET##INS#•#INS##RET#\\end{equation}•\n\\boxed{#INS#•#INS#}\n\\begin{\n";

    #[test]
    fn bottom_up_order_like_texshop() {
        let t = Table::parse(SAMPLE);
        let c: Vec<_> = t.candidates("{eq").iter().map(|e| e.body.as_str()).collect();
        assert!(c[0].starts_with("{equation}"));
        assert!(c[1].starts_with("{eqnarray}"));
        assert!(c[2].starts_with("{eqnarray*}"));
        assert_eq!(t.candidates("\\b")[0].body, "\\begin{");
    }

    #[test]
    fn abbreviations_match_exactly() {
        let t = Table::parse(SAMPLE);
        assert_eq!(t.candidates("\\bsk")[0].body, "\\bigskip");
        assert!(t.candidates("\\bs").iter().all(|e| e.key.is_none()));
    }

    #[test]
    fn expands_markup_with_indent() {
        let e = expand("{equation}#RET##INS#•#INS##RET#\\end{equation}•", "  ");
        assert_eq!(e.text, "{equation}\n  •\n  \\end{equation}•");
        assert_eq!((e.sel_start, e.sel_len), (13, 1));
        let e = expand("\\frac{#INS#•‹num›#INS#}{•‹den›}", "");
        assert_eq!((e.sel_start, e.sel_len), (6, 6));
        let e = expand("\\bigskip", "");
        assert_eq!((e.sel_start, e.sel_len), (8, 0));
    }

    #[test]
    fn word_boundaries() {
        assert_eq!(word_before("text \\sec", 9), "\\sec");
        assert_eq!(word_before("\\begin{eq", 9), "{eq");
        assert_eq!(word_before("  benu", 6), "benu");
        assert_eq!(word_before("a,b", 3), "b");
    }

    #[test]
    fn parses_shipped_texshop_file_if_present() {
        let p = Path::new("/Applications/TeX/TeXShop.app/Contents/Resources/TeXShop/CommandCompletion/CommandCompletion.txt");
        if let Ok(l) = load(p) {
            assert!(l.table.entries.len() > 500);
            assert_eq!(l.table.candidates("\\b")[0].body, "\\begin{");
            assert!(l.table.candidates("{eq")[0].body.starts_with("{equation}"));
        }
    }
}
