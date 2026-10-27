//! The 116-character / 40% line rule, read from the compiled PDF (a port of
//! `auto-apply/check_lines.py`, so both agree): each printed line is at most
//! 116 characters, and when a bullet or Skills entry wraps, its later lines
//! are at least 47 characters. Over-long lines already in the base resume
//! are reported but not counted as failures.

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MAX: usize = 116;
pub const MIN_WRAP: usize = 47;
const SKILLS_END: &[&str] = &["Experience", "Projects", "Education"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Violation {
    pub kind: String, // "over" | "short-wrap"
    pub length: usize,
    pub line: String,
    /// Start of the bullet the line belongs to.
    pub bullet: String,
    /// Already in the base resume (reported, not failed).
    pub in_base: bool,
}

impl Violation {
    pub fn describe(&self) -> String {
        match self.kind.as_str() {
            "over" => format!(
                "printed line is {} characters (max {MAX}): \"{}\" (bullet starts \"{}\")",
                self.length, self.line, self.bullet
            ),
            _ => format!(
                "wrapped line is only {} characters (min {MIN_WRAP}): \"{}\" (bullet starts \"{}\"); \
                 shorten the bullet to one line or lengthen it so the last line is longer",
                self.length, self.line, self.bullet
            ),
        }
    }
}

pub fn pdftotext() -> Option<PathBuf> {
    ["/opt/homebrew/bin/pdftotext", "/usr/local/bin/pdftotext", "/Library/TeX/texbin/pdftotext"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

fn pdf_lines(pdf: &Path) -> Result<Vec<String>, String> {
    let bin = pdftotext().ok_or("pdftotext not found (brew install poppler)")?;
    let out = Command::new(bin).args(["-layout"]).arg(pdf).arg("-").output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("pdftotext failed on {}", pdf.display()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(String::from).collect())
}

fn clean(s: &str) -> String {
    s.trim().trim_start_matches('•').split_whitespace().collect::<Vec<_>>().join(" ")
}

fn indent(s: &str) -> usize {
    s.len() - s.trim_start_matches(' ').len()
}

fn is_skills_label(s: &str) -> bool {
    let Some((label, _)) = s.split_once(':') else { return false };
    let mut chars = label.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase()) && chars.all(|c| c.is_ascii_alphabetic() || c == ' ')
}

/// Group printed lines into bullets and Skills entries.
pub fn groups(lines: &[String]) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut in_skills = false;
    let mut cur: Option<(bool, Vec<String>)> = None; // (is_skills, lines)
    let mut cur_indent = 0;
    let flush = |cur: &mut Option<(bool, Vec<String>)>, out: &mut Vec<Vec<String>>| {
        if let Some((_, l)) = cur.take() {
            out.push(l);
        }
    };
    for raw in lines {
        let s = raw.trim();
        if s == "Skills" {
            in_skills = true;
            continue;
        }
        if SKILLS_END.contains(&s) {
            in_skills = false;
        }
        if s.is_empty() {
            flush(&mut cur, &mut out);
            continue;
        }
        if s.starts_with('•') {
            flush(&mut cur, &mut out);
            cur = Some((false, vec![clean(s)]));
            cur_indent = indent(raw);
        } else if in_skills && is_skills_label(s) {
            flush(&mut cur, &mut out);
            cur = Some((true, vec![clean(s)]));
            cur_indent = indent(raw);
        } else if let Some((is_skills, l)) = cur.as_mut().filter(|(sk, _)| *sk || indent(raw) > cur_indent) {
            let _ = is_skills;
            l.push(clean(s));
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

pub fn check_lines(lines: &[String], base_lines: &HashSet<String>) -> Vec<Violation> {
    let mut v = Vec::new();
    for g in groups(lines) {
        let bullet: String = g[0].chars().take(50).collect();
        for (i, line) in g.iter().enumerate() {
            let n = line.chars().count();
            if n > MAX {
                v.push(Violation { kind: "over".into(), length: n, line: line.clone(), bullet: bullet.clone(), in_base: base_lines.contains(line) });
            }
            if i > 0 && n < MIN_WRAP {
                v.push(Violation { kind: "short-wrap".into(), length: n, line: line.clone(), bullet: bullet.clone(), in_base: false });
            }
        }
    }
    v
}

/// How a compiled resume prints: each bullet / Skills entry as its printed lines.
pub fn printed(pdf: &Path) -> Result<Vec<Vec<String>>, String> {
    Ok(groups(&pdf_lines(pdf)?))
}

/// Check a compiled resume; `base` makes pre-existing long lines non-fatal.
pub fn check(pdf: &Path, base: Option<&Path>) -> Result<Vec<Violation>, String> {
    let base_lines: HashSet<String> = match base.filter(|b| b.is_file()) {
        Some(b) => pdf_lines(b)?.iter().map(|l| clean(l)).collect(),
        None => HashSet::new(),
    };
    Ok(check_lines(&pdf_lines(pdf)?, &base_lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn flags_long_and_short_wraps() {
        let long = "x".repeat(120);
        let text = format!(
            "Skills\nTechnologies: React, Docker, FastAPI, PostgreSQL, Google Cloud, Supabase, pgvector, Claude Code,\n  RAG\n\nExperience\n\n   • {long}\n   • Short bullet that wraps\n        ok\n"
        );
        let v = check_lines(&lines(&text), &HashSet::new());
        let kinds: Vec<(&str, usize)> = v.iter().map(|x| (x.kind.as_str(), x.length)).collect();
        assert_eq!(kinds, vec![("short-wrap", 3), ("over", 120), ("short-wrap", 2)]);
    }

    #[test]
    fn base_long_lines_are_marked() {
        let long = "y".repeat(118);
        let base: HashSet<String> = [long.clone()].into_iter().collect();
        let v = check_lines(&lines(&format!("   • {long}\n")), &base);
        assert!(v[0].in_base);
    }
}
