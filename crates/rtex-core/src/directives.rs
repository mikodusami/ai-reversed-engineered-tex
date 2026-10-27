//! `% !TEX key = value` magic comments.
//!
//! TeXShop recognises these in the first lines of a source file (see the
//! `% !TEX ...` string table in the TeXShop binary, documented in
//! docs/RE-NOTES.md). We accept the same spellings: with or without a space
//! after `%`, `TS-program` and `program` as synonyms, case-insensitive keys.

use serde::Serialize;

/// How many leading lines are scanned for directives.
pub const SCAN_LINES: usize = 20;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Directives {
    /// `% !TEX root = ../main.tex`
    pub root: Option<String>,
    /// `% !TEX TS-program = xelatex` (or `program`)
    pub program: Option<String>,
    /// `% !TEX encoding = UTF-8 Unicode`
    pub encoding: Option<String>,
    /// `% !TEX parameter = ...` (or `TS-parameter`): passed to engine scripts as `$2`
    pub parameter: Option<String>,
    /// `% !BIB TS-program = biber`
    pub bib_program: Option<String>,
    /// `% !TEX spellcheck = en_US`
    pub spellcheck: Option<String>,
    /// Bare flags such as `useTabs`, `pdfSinglePage`, `noPreview`.
    pub flags: Vec<String>,
}

impl Directives {
    pub fn parse(text: &str) -> Directives {
        let mut d = Directives::default();
        for line in text.lines().take(SCAN_LINES) {
            let Some(rest) = line.trim_start().strip_prefix('%') else { continue };
            let rest = rest.trim_start();
            let (is_bib, body) = if let Some(b) = strip_prefix_ci(rest, "!TEX") {
                (false, b)
            } else if let Some(b) = strip_prefix_ci(rest, "!BIB") {
                (true, b)
            } else {
                continue;
            };
            // Must be followed by whitespace (or `-` for `!TEX-noPreview`).
            let body = match body.chars().next() {
                Some(c) if c.is_whitespace() => body.trim(),
                Some('-') => {
                    d.flags.push(body[1..].trim().to_string());
                    continue;
                }
                _ => continue,
            };
            match body.split_once('=') {
                Some((key, value)) => {
                    let key = key.trim().to_ascii_lowercase();
                    let value = value.trim().to_string();
                    if value.is_empty() {
                        continue;
                    }
                    let slot = match (is_bib, key.as_str()) {
                        (true, "ts-program" | "program") => &mut d.bib_program,
                        (false, "root") => &mut d.root,
                        (false, "ts-program" | "program") => &mut d.program,
                        (false, "encoding") => &mut d.encoding,
                        (false, "parameter" | "ts-parameter") => &mut d.parameter,
                        (false, "spellcheck") => &mut d.spellcheck,
                        _ => continue,
                    };
                    // First occurrence wins, as in TeXShop.
                    if slot.is_none() {
                        *slot = Some(value);
                    }
                }
                None if !is_bib && !body.is_empty() => d.flags.push(body.to_string()),
                None => {}
            }
        }
        d
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_texshop_spellings() {
        let src = "% !TEX TS-program = xelatex\n%!TEX root = ../main.tex\n\
                   % !TEX encoding = UTF-8 Unicode\n% !BIB TS-program = biber\n\
                   % !TEX parameter = --shell-escape\n% !TEX useTabs\n\\documentclass{article}\n";
        let d = Directives::parse(src);
        assert_eq!(d.program.as_deref(), Some("xelatex"));
        assert_eq!(d.root.as_deref(), Some("../main.tex"));
        assert_eq!(d.encoding.as_deref(), Some("UTF-8 Unicode"));
        assert_eq!(d.bib_program.as_deref(), Some("biber"));
        assert_eq!(d.parameter.as_deref(), Some("--shell-escape"));
        assert_eq!(d.flags, vec!["useTabs"]);
    }

    #[test]
    fn program_synonym_and_first_wins() {
        let d = Directives::parse("% !tex program = lualatex\n% !TEX TS-program = pdflatex\n");
        assert_eq!(d.program.as_deref(), Some("lualatex"));
    }

    #[test]
    fn ignores_late_and_non_directives() {
        let mut src = String::from("% just a comment !TEX root = x.tex\n");
        for _ in 0..SCAN_LINES {
            src.push_str("text\n");
        }
        src.push_str("% !TEX root = late.tex\n");
        assert_eq!(Directives::parse(&src).root, None);
    }

    #[test]
    fn dash_flags() {
        let d = Directives::parse("%!TEX-noPreview\n");
        assert_eq!(d.flags, vec!["noPreview"]);
    }
}
