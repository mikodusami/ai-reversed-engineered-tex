//! TeX log parser (TeXShop's equivalent is `TSLogWindow` plus the bundled
//! `texloganalyser` script; see docs/RE-NOTES.md).
//!
//! The parser runs as one linear pass over the log. It works best with logs
//! written with `max_print_line` raised (rtex's builder sets that in the
//! environment) and `-file-line-error`, but it falls back to tracking the
//! open-file stack through `(` and `)` for logs from other front-ends.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
    BadBox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub level: Level,
    /// Source file as TeX printed it (e.g. `./chapters/one.tex`).
    pub file: Option<String>,
    pub line: Option<u32>,
    pub message: String,
    /// For errors: the `l.N ...` context TeX printed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct LogSummary {
    pub messages: Vec<Message>,
    /// LaTeX (or a package) asked for another pass.
    pub rerun_latex: bool,
    /// biblatex asked for biber / bibtex.
    pub rerun_biber: bool,
    pub rerun_bibtex: bool,
    pub undefined_references: bool,
    pub output_file: Option<String>,
    pub pages: Option<u32>,
    pub fatal: bool,
}

impl LogSummary {
    pub fn errors(&self) -> impl Iterator<Item = &Message> {
        self.messages.iter().filter(|m| m.level == Level::Error)
    }
    pub fn count(&self, level: Level) -> usize {
        self.messages.iter().filter(|m| m.level == level).count()
    }
}

/// Parse a whole log.
pub fn parse(log: &str) -> LogSummary {
    let mut s = LogSummary::default();
    let lines: Vec<&str> = log.lines().collect();
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;

        if let Some((file, lno, msg)) = file_line_error(line) {
            let (ctx, ctx_line, used) = error_context(&lines[i..]);
            i += used;
            s.messages.push(Message {
                level: Level::Error,
                file: Some(file.to_string()),
                line: Some(lno).or(ctx_line),
                message: msg.to_string(),
                context: ctx,
            });
            continue;
        }
        if let Some(msg) = line.strip_prefix("! ") {
            if msg.contains("==> Fatal error occurred") || msg.starts_with("Emergency stop") {
                s.fatal = true;
            }
            let (ctx, ctx_line, used) = error_context(&lines[i..]);
            i += used;
            s.messages.push(Message {
                level: Level::Error,
                file: current_file(&stack),
                line: ctx_line,
                message: msg.to_string(),
                context: ctx,
            });
            continue;
        }
        if line.starts_with("!  ==> Fatal error occurred") || line.contains("==> Fatal error occurred") {
            s.fatal = true;
        }

        if let Some(rest) = line.strip_prefix("Output written on ") {
            if let Some((file, tail)) = rest.rsplit_once(" (") {
                s.output_file = Some(file.to_string());
                s.pages = tail.split_whitespace().next().and_then(|n| n.parse().ok());
            }
        }

        // Bad boxes: `Overfull \hbox (12.0pt too wide) in paragraph at lines 10--12`
        if (line.starts_with("Overfull \\") || line.starts_with("Underfull \\")) && line.len() > 12 {
            let lno = after(line, "at lines ")
                .or_else(|| after(line, "at line "))
                .and_then(leading_number);
            s.messages.push(Message {
                level: Level::BadBox,
                file: current_file(&stack),
                line: lno,
                message: line.trim().to_string(),
                context: None,
            });
            continue;
        }

        if let Some((pkg, first)) = warning_start(line) {
            // Continuation lines are indented to line up with the message, or
            // start with `(pkgname)`.
            let mut text = first.trim().to_string();
            let cont_prefix = pkg.map(|p| format!("({p})"));
            while i < lines.len() {
                let next = lines[i];
                let is_cont = match &cont_prefix {
                    Some(pre) => next.starts_with(pre.as_str()),
                    None => next.starts_with("               ") && !next.trim().is_empty(),
                };
                if !is_cont {
                    break;
                }
                let body = match &cont_prefix {
                    Some(pre) => &next[pre.len()..],
                    None => next,
                };
                text.push(' ');
                text.push_str(body.trim());
                i += 1;
            }
            classify_rerun(&mut s, pkg, &text);
            let lno = after(&text, "on input line ").and_then(leading_number);
            s.messages.push(Message {
                level: Level::Warning,
                file: current_file(&stack),
                line: lno,
                message: match pkg {
                    Some(p) => format!("{p}: {text}"),
                    None => text,
                },
                context: None,
            });
            continue;
        }

        if line.contains("Rerun to get") || line.contains("Please rerun LaTeX") {
            s.rerun_latex = true;
        }
        track_files(line, &mut stack);
    }
    s
}

fn classify_rerun(s: &mut LogSummary, pkg: Option<&str>, text: &str) {
    let lower = text.to_ascii_lowercase();
    if lower.contains("rerun to get")
        || lower.contains("label(s) may have changed")
        || lower.contains("please rerun latex")
        || (pkg == Some("rerunfilecheck") && lower.contains("has changed"))
        || lower.contains("rerun latex")
    {
        s.rerun_latex = true;
    }
    if lower.contains("please (re)run biber") || lower.contains("please rerun biber") {
        s.rerun_biber = true;
    }
    if lower.contains("please (re)run bibtex") {
        s.rerun_bibtex = true;
    }
    if lower.contains("there were undefined references") || lower.contains("citation") && lower.contains("undefined") {
        s.undefined_references = true;
    }
}

/// `LaTeX Warning: ...`, `Package foo Warning: ...`, `Class bar Warning: ...`,
/// `LaTeX Font Warning: ...`, `pdfTeX warning: ...`.
fn warning_start(line: &str) -> Option<(Option<&str>, &str)> {
    if let Some(rest) = line.strip_prefix("LaTeX Warning: ") {
        return Some((None, rest));
    }
    if let Some(rest) = line.strip_prefix("LaTeX Font Warning: ") {
        return Some((Some("Font"), rest));
    }
    for kind in ["Package ", "Class ", "Module "] {
        if let Some(rest) = line.strip_prefix(kind) {
            if let Some((name, msg)) = rest.split_once(" Warning: ") {
                if !name.contains(' ') {
                    return Some((Some(name), msg));
                }
            }
        }
    }
    if let Some(idx) = line.find("pdfTeX warning") {
        if let Some((_, msg)) = line[idx..].split_once(": ") {
            return Some((Some("pdfTeX"), msg));
        }
    }
    None
}

/// `./file.tex:12: message` (`-file-line-error` style).
fn file_line_error(line: &str) -> Option<(&str, u32, &str)> {
    // Fast reject: the file part must look like a path.
    let first = line.as_bytes().first()?;
    if !(*first == b'.' || *first == b'/' || first.is_ascii_alphanumeric()) {
        return None;
    }
    let colon = line.find(':')?;
    let (file, rest) = (&line[..colon], &line[colon + 1..]);
    if file.is_empty() || file.contains(' ') && !file.contains('/') || !file.contains('.') {
        return None;
    }
    let colon2 = rest.find(':')?;
    let lno: u32 = rest[..colon2].parse().ok()?;
    let msg = rest[colon2 + 1..].trim();
    Some((file, lno, msg))
}

/// After an error, TeX prints the context ending in a line `l.<n> ...`.
/// Returns (context, line number, lines consumed).
fn error_context(rest: &[&str]) -> (Option<String>, Option<u32>, usize) {
    for (k, l) in rest.iter().enumerate().take(12) {
        if let Some(t) = l.strip_prefix("l.") {
            let n = leading_number(t);
            // `l.N <text before the error point>`, then the rest of the
            // input line on the next line, indented. Join them with a marker.
            let digits = t.bytes().take_while(|b| b.is_ascii_digit()).count();
            let mut ctx = t[digits..].trim().to_string();
            let mut used = k + 1;
            if let Some(next) = rest.get(k + 1) {
                if next.starts_with(' ') && !next.trim().is_empty() {
                    ctx.push_str(" ▸ ");
                    ctx.push_str(next.trim());
                    used += 1;
                }
            }
            return (Some(ctx), n, used);
        }
        if l.starts_with("! ") || file_line_error(l).is_some() {
            break;
        }
    }
    (None, None, 0)
}

/// Maintain the stack of open files from TeX's `(file` ... `)` output.
fn track_files(line: &str, stack: &mut Vec<Option<String>>) {
    let b = line.as_bytes();
    let mut k = 0;
    while k < b.len() {
        match b[k] {
            b'(' => {
                let start = k + 1;
                let mut end = start;
                while end < b.len() && !matches!(b[end], b' ' | b'(' | b')' | b'[' | b'{' | b'"') {
                    end += 1;
                }
                let name = &line[start..end];
                let looks_like_file = name.len() > 1
                    && (name.starts_with("./") || name.starts_with('/') || name.starts_with("../"))
                    || (name.contains('.') && name.rsplit('.').next().is_some_and(|e| (1..=4).contains(&e.len()) && e.bytes().all(|c| c.is_ascii_alphabetic())));
                stack.push(looks_like_file.then(|| name.to_string()));
                k = end;
            }
            b')' => {
                stack.pop();
                k += 1;
            }
            _ => k += 1,
        }
    }
}

fn current_file(stack: &[Option<String>]) -> Option<String> {
    stack
        .iter()
        .rev()
        .flatten()
        .find(|f| is_source_ext(f))
        .or_else(|| stack.iter().rev().flatten().next())
        .cloned()
}

fn is_source_ext(f: &str) -> bool {
    [".tex", ".ltx", ".dtx", ".Rnw", ".sty", ".cls", ".bbl", ".ind"].iter().any(|e| f.ends_with(e))
}

fn after<'a>(s: &'a str, pat: &str) -> Option<&'a str> {
    s.find(pat).map(|i| &s[i + pat.len()..])
}

fn leading_number(s: &str) -> Option<u32> {
    let end = s.bytes().position(|c| !c.is_ascii_digit()).unwrap_or(s.len());
    s[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = r#"This is pdfTeX, Version 3.141592653-2.6-1.40.28 (TeX Live 2026) (preloaded format=pdflatex)
**broken.tex
(./broken.tex
LaTeX2e <2025-11-01>
(/usr/local/texlive/2026/texmf-dist/tex/latex/base/article.cls
Document Class: article 2025/01/22 v1.4n Standard LaTeX document class
(/usr/local/texlive/2026/texmf-dist/tex/latex/base/size10.clo
File: size10.clo 2025/01/22 v1.4n Standard LaTeX file (size option)
)
) (./sub/part.tex
./broken.tex:4: Undefined control sequence.
l.4 \undefinedmacro
                    here.
The control sequence at the end of the top line
Overfull \hbox (137.88913pt too wide) detected at line 5
\OT1/cmr/m/n/10 This is a much too wide box of text
 []
Underfull \hbox (badness 10000) in paragraph at lines 7--9
LaTeX Warning: Reference `nolabel' on page 1 undefined on input line 6.
)
Package hyperref Warning: Token not allowed in a PDF string (Unicode):
(hyperref)                removing `math shift' on input line 12.
Package rerunfilecheck Warning: File `x.out' has changed.
(rerunfilecheck)                Rerun to get outlines right
(rerunfilecheck)                or use package `bookmark'.
LaTeX Warning: There were undefined references.
LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right.
Output written on broken.pdf (1 page, 18734 bytes).
"#;

    #[test]
    fn parses_messages() {
        let s = parse(LOG);
        let errs: Vec<_> = s.errors().collect();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].file.as_deref(), Some("./broken.tex"));
        assert_eq!(errs[0].line, Some(4));
        assert_eq!(errs[0].message, "Undefined control sequence.");
        assert!(errs[0].context.as_deref().unwrap().contains("\\undefinedmacro"));
        assert_eq!(s.count(Level::BadBox), 2);
        let bad: Vec<_> = s.messages.iter().filter(|m| m.level == Level::BadBox).collect();
        assert_eq!(bad[0].line, Some(5));
        assert_eq!(bad[1].line, Some(7));
        assert_eq!(bad[1].file.as_deref(), Some("./sub/part.tex"));
        let w: Vec<_> = s.messages.iter().filter(|m| m.level == Level::Warning).collect();
        assert_eq!(w[0].line, Some(6));
        assert!(w[1].message.starts_with("hyperref: Token not allowed"));
        assert_eq!(w[1].line, Some(12));
        assert!(s.rerun_latex);
        assert!(s.undefined_references);
        assert_eq!(s.pages, Some(1));
        assert_eq!(s.output_file.as_deref(), Some("broken.pdf"));
    }

    #[test]
    fn classic_bang_error() {
        let log = "(./a.tex\n! Missing $ inserted.\n<inserted text> \n                $\nl.7 x^\n       2\n)\n";
        let s = parse(log);
        let e = s.errors().next().unwrap();
        assert_eq!(e.line, Some(7));
        assert_eq!(e.file.as_deref(), Some("./a.tex"));
        assert_eq!(e.message, "Missing $ inserted.");
    }

    #[test]
    fn biber_request() {
        let s = parse("Package biblatex Warning: Please (re)run Biber on the file:\n(biblatex)                paper\n(biblatex)                and rerun LaTeX afterwards.\n");
        assert!(s.rerun_biber);
    }
}
