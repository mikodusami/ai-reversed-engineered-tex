//! A deterministic check on what the tailored resume claims: every Skills
//! term that is new compared to base.tex must appear somewhere in the
//! evidence sent with the request (the resumes and background files). Terms that don't are
//! listed in the review for you to confirm. Claude is told never to invent
//! skills; this catches the borderline cases it talks itself into.

use std::collections::BTreeSet;

/// Comma-separated terms in the `\section*{Skills}` block, LaTeX stripped.
pub fn skills_terms(tex: &str) -> BTreeSet<String> {
    let Some(start) = tex.find("\\section*{Skills}") else { return BTreeSet::new() };
    let rest = &tex[start + "\\section*{Skills}".len()..];
    let end = rest.find("\\section").unwrap_or(rest.len());
    let block = &rest[..end];
    let mut out = BTreeSet::new();
    for line in block.lines() {
        let line = line.split('%').next().unwrap_or("");
        // Drop the bold label (`\textbf{Technologies:}`), commands and line breaks.
        let line = regex_lite_strip(line);
        for term in line.split(',') {
            let t = term.trim().trim_end_matches('.').trim();
            if t.len() > 1 && !t.ends_with(':') {
                out.insert(t.to_string());
            }
        }
    }
    out
}

fn regex_lite_strip(line: &str) -> String {
    let re_label = regex::Regex::new(r"\\textbf\{[^}]*\}").unwrap();
    let re_cmd = regex::Regex::new(r"\\(newline|vspace\{[^}]*\}|hfill|noindent)").unwrap();
    let s = re_label.replace_all(line, "");
    let s = re_cmd.replace_all(&s, "");
    s.replace("\\\\", "").replace("\\&", "&").replace("\\#", "#").replace("\\%", "%")
}

/// New Skills terms with no support in the verified resumes.
pub fn unverified_skills(base: &str, tailored: &str, verified_sources: &[String]) -> Vec<String> {
    let before = skills_terms(base);
    let haystack = verified_sources.join("\n").to_lowercase();
    skills_terms(tailored)
        .into_iter()
        .filter(|t| !before.contains(t))
        .filter(|t| !haystack.contains(&t.to_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "\\section*{Skills}\n\\textbf{Technologies:}\nReact.js, PostgreSQL,\nDocker \\\\\n\\section*{Experience}\n\\item Used Node once";

    #[test]
    fn terms() {
        let t = skills_terms(BASE);
        assert_eq!(t.into_iter().collect::<Vec<_>>(), vec!["Docker", "PostgreSQL", "React.js"]);
    }

    #[test]
    fn flags_unsupported_additions() {
        let tailored = BASE.replace("React.js,", "React, Node.js, Jest, Kubernetes,");
        let detailed = "Testing: Jest, pytest. Built with React.".to_string();
        let v = unverified_skills(BASE, &tailored, &[BASE.to_string(), detailed]);
        assert_eq!(v, vec!["Kubernetes", "Node.js"]);
    }
}
