//! Apply Claude's original→replacement edits to a copy of the base resume.
//!
//! An edit's `original` should be copied verbatim from the base resume. When it is
//! not (re-flowed whitespace, a missing indent), it is matched as a sequence
//! of tokens separated by any whitespace. An edit that matches nowhere, or in
//! more than one place, is reported instead of guessed.

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Edit {
    /// "replace" (default), "delete" or "insert_after".
    pub op: String,
    pub section: String,
    pub entry: String,
    pub original: String,
    pub replacement: String,
    pub rationale: String,
    pub keywords: Vec<String>,
}

impl Edit {
    pub fn op(&self) -> &str {
        match self.op.as_str() {
            "delete" | "insert_after" => &self.op,
            _ => "replace",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    pub tex: String,
    /// (edit index, problem) for edits that could not be applied.
    pub problems: Vec<(usize, String)>,
    pub applied: usize,
}

pub fn apply(base: &str, edits: &[Edit]) -> Applied {
    let mut tex = base.to_string();
    let mut problems = Vec::new();
    let mut applied = 0;
    for (i, e) in edits.iter().enumerate() {
        let original = e.original.trim();
        if original.is_empty() {
            problems.push((i, "edit has an empty `original`".into()));
            continue;
        }
        if e.op() == "replace" && original == e.replacement.trim() {
            continue;
        }
        match locate(&tex, original) {
            Ok((start, end)) if e.op() == "delete" => {
                tex.replace_range(start..end, "");
                applied += 1;
            }
            Ok((_, end)) if e.op() == "insert_after" => {
                let line_start = tex[..end].rfind('\n').map(|p| p + 1).unwrap_or(0);
                let indent: String = tex[line_start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                let block = reindent(e.replacement.trim(), &indent);
                tex.insert_str(end, &format!("\n\n{indent}{block}"));
                applied += 1;
            }
            Ok((start, end)) => {
                // Keep the indentation of the line being replaced.
                let line_start = tex[..start].rfind('\n').map(|p| p + 1).unwrap_or(0);
                let indent: String = tex[line_start..start].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                let at_line_start = tex[line_start..start].trim().is_empty();
                let replacement = reindent(e.replacement.trim(), if at_line_start { &indent } else { "" });
                tex.replace_range(start..end, &replacement);
                applied += 1;
            }
            Err(why) => problems.push((i, why)),
        }
    }
    Applied { tex: tidy_blank_lines(&tex), problems, applied }
}

/// Byte range of `needle` in `hay`: exact first, then whitespace-flexible.
fn locate(hay: &str, needle: &str) -> Result<(usize, usize), String> {
    let exact: Vec<usize> = hay.match_indices(needle).map(|(i, _)| i).collect();
    match exact.len() {
        1 => return Ok((exact[0], exact[0] + needle.len())),
        n if n > 1 => return Err(format!("`original` appears {n} times in the base resume; make it unique")),
        _ => {}
    }
    let tokens: Vec<String> = needle.split_whitespace().map(regex::escape).collect();
    let re = Regex::new(&tokens.join(r"\s+")).map_err(|e| e.to_string())?;
    let found: Vec<(usize, usize)> = re.find_iter(hay).map(|m| (m.start(), m.end())).collect();
    match found.len() {
        1 => Ok(found[0]),
        0 => Err(format!("`original` was not found in the base resume: {}", preview(needle))),
        n => Err(format!("`original` matches {n} places in the base resume; make it unique: {}", preview(needle))),
    }
}

/// Indent every line after the first with `indent` (the first line inherits
/// the indentation already in front of the match).
fn reindent(text: &str, indent: &str) -> String {
    let mut lines = text.lines();
    let mut out = lines.next().unwrap_or("").to_string();
    for l in lines {
        out.push('\n');
        if !l.trim().is_empty() && !l.starts_with(' ') && !l.starts_with('\t') {
            out.push_str(indent);
        }
        out.push_str(l);
    }
    out
}

/// Deleting an entry leaves blank lines behind; keep at most one in a row.
fn tidy_blank_lines(tex: &str) -> String {
    let mut out = String::with_capacity(tex.len());
    let mut blanks = 0;
    for line in tex.split_inclusive('\n') {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(line);
    }
    out
}

fn preview(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 90 {
        format!("{}…", one.chars().take(90).collect::<String>())
    } else {
        one
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "\\section*{Skills}\n\n\\textbf{Technologies:}\nReact.js, React Native, MySQL,\nGoogle Cloud \\\\\n\n\\begin{itemize}\n    \\item Built a React.js form that cut load time from 290 to 170 ms.\n    \\item Wrote tests.\n\\end{itemize}\n";

    fn e(original: &str, replacement: &str) -> Edit {
        Edit { original: original.into(), replacement: replacement.into(), ..Default::default() }
    }

    #[test]
    fn exact_and_indent_kept() {
        let a = apply(BASE, &[e("\\item Built a React.js form that cut load time from 290 to 170 ms.", "\\item Built a react.js form, 290 → 170 ms.")]);
        assert!(a.problems.is_empty());
        assert!(a.tex.contains("    \\item Built a react.js form, 290 → 170 ms.\n"));
    }

    #[test]
    fn whitespace_flexible_multiline_skills() {
        let a = apply(BASE, &[e("\\textbf{Technologies:} React.js, React Native, MySQL, Google Cloud \\\\", "\\textbf{Technologies:}\nreact.js, React Native, MySQL, Google Cloud \\\\")]);
        assert_eq!(a.applied, 1, "{:?}", a.problems);
        assert!(a.tex.contains("\\textbf{Technologies:}\nreact.js, React Native"));
    }

    const PROJECTS: &str = "\\section*{Projects}\n\n\\textbf{Old Project} \\\\\n\\begin{itemize}\n    \\item Did old things.\n\\end{itemize}\n\n\\textbf{Kept Project} \\\\\n\\begin{itemize}\n    \\item Kept.\n\\end{itemize}\n";

    #[test]
    fn delete_and_insert_entries() {
        let del = Edit { op: "delete".into(), original: "\\textbf{Old Project} \\\\\n\\begin{itemize}\n    \\item Did old things.\n\\end{itemize}".into(), ..Default::default() };
        let add = Edit {
            op: "insert_after".into(),
            original: "\\textbf{Kept Project} \\\\\n\\begin{itemize}\n    \\item Kept.\n\\end{itemize}".into(),
            replacement: "\\textbf{Scraper} \\\\\n\\begin{itemize}\n    \\item Built an async scraper.\n\\end{itemize}".into(),
            ..Default::default()
        };
        let a = apply(PROJECTS, &[del, add]);
        assert_eq!(a.applied, 2, "{:?}", a.problems);
        assert!(!a.tex.contains("Old Project"));
        assert!(!a.tex.contains("\n\n\n"), "{}", a.tex);
        let kept = a.tex.find("Kept Project").unwrap();
        let new = a.tex.find("Scraper").unwrap();
        assert!(new > kept, "{}", a.tex);
        assert!(a.tex.contains("    \\item Built an async scraper."), "{}", a.tex);
    }

    #[test]
    fn reports_missing_and_ambiguous() {
        let a = apply(BASE, &[e("\\item Never written.", "x"), e("\\item", "y")]);
        assert_eq!(a.applied, 0);
        assert!(a.problems[0].1.contains("not found"));
        assert!(a.problems[1].1.contains("2 times"));
    }
}
