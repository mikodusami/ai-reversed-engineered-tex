//! The tailoring prompt. It mirrors how tailoring works in the Claude web
//! app: your own instruction set as the system prompt (verbatim, Part 1 of
//! `auto-apply/tailoring.md`), the context files in full, then the posting.
//! The only additions are an output contract so the answer can be applied
//! mechanically, and the evidence rules from Part 3 of the same file.

use crate::workspace::{Job, Workspace};
use serde_json::{json, Value};

/// Part 1 of tailoring.md (your instructions, as written), or the whole file
/// if it has no parts.
pub fn instructions(ws: &Workspace) -> String {
    let text = std::fs::read_to_string(ws.tailoring_md()).unwrap_or_default();
    section(&text, "## Part 1").unwrap_or(text)
}

/// Part 3 (evidence rules), if present.
pub fn evidence_rules(ws: &Workspace) -> Option<String> {
    let text = std::fs::read_to_string(ws.tailoring_md()).ok()?;
    section(&text, "## Part 3")
}

/// Body of the `## ...` section whose heading starts with `heading`.
fn section(text: &str, heading: &str) -> Option<String> {
    let start = text.find(heading)?;
    let after = &text[start..];
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
    let rest = &after[body_start..];
    let end = rest.find("\n## ").map(|i| i + 1).unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

/// What every request needs to know about how its answer is used.
const CONTRACT_COMMON: &str = r#"## How this request runs

This is the [tailor] workflow above, run by RTeX on my Mac (no outreach email). RTeX applies your edits
to a copy of the base resume, compiles it and checks the printed lines. You never rewrite the whole resume:
return only the edits, each one a targeted change to the base resume.

Edit operations (`op`):
- "replace": `original` is copied character for character from the base resume and `replacement` is the LaTeX
  that takes its place. Use it to reword a bullet, rewrite a Skills entry, or swap one project for another.
- "delete": remove `original` (for example a whole project entry that fits this role badly). `replacement` is "".
- "insert_after": add `replacement` (for example a new project entry, in the base's exact format) right after
  `original`, which must be a complete existing entry.

What `original` can be: one whole `\item ...` bullet; one whole Skills entry (the `\textbf{...:}` line and
the lines after it, up to and including its `\\`); or one whole Experience/Projects entry (from its
`\textbf{...}` heading line through its `\end{itemize}`). Never edit the preamble, section headings, spacing
commands, dates or the order of entries you keep.

Rules the compiled PDF is checked against:
- Printed lines: at most 116 characters each; when a bullet or Skills entry wraps, its last line is at least
  47 characters. Reword to fit; never change fonts or spacing. The resume must stay one page.
- Keywords: use the posting's exact spelling and capitalization wherever my background truthfully supports the
  term, including in Skills. Never add a skill, tool, metric, title or outcome I do not have.
- Only change what improves alignment with the role and the ATS screen; leave well-aligned bullets alone.
- Always tailor: set `decision` to "tailor". I decide which jobs to apply to."#;

/// When there is more than one base resume (base.tex and base-data.tex).
fn contract_bases(bases: &[String]) -> Option<String> {
    if bases.len() < 2 {
        return None;
    }
    let names: Vec<String> = bases.iter().map(|b| format!("`{}`", base_name(b))).collect();
    Some(format!(
        "Base resumes: {}. Choose one as step 0 says (base-data.tex when the role leans towards data, \
         otherwise base.tex), set `base` to its file name, and copy every `original` from that file.",
        names.join(" and ")
    ))
}

/// `resumes/base-data.tex` → `base-data.tex`.
pub fn base_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// Optional (`hard_gates` setting): stop before tailoring jobs that clearly don't fit.
const CONTRACT_GATES: &str = r#"Exception: set `decision` to "stop" when the posting has a hard requirement I clearly fail (for example a
security clearance I don't hold, far more years of experience than I have, a degree I don't have, a salary
maximum below $90,000, or a contract/C2C role). Then explain why in `stop_reason` and return no edits."#;

const CONTRACT_BRIEF: &str = r#"Work out the role, the business problem, the stack and what an ATS will most likely screen for
(steps 6 to 10), but do not write that analysis out: I only need the edits. Each edit's `rationale` is one
short sentence naming the requirement or ATS keyword it targets."#;

const CONTRACT_ANALYSIS: &str = r#"Also write the analysis in `analysis`, in Markdown, with these headings: Strategic role analysis,
Experience mapping, ATS keywords (a table: keyword in the posting's spelling, where it appears now, what you
did), Honest fit assessment, Strategic positioning."#;

/// The system prompt: your instructions, how the answer is used, and your
/// whole background. It is identical for every job, so Claude's prompt
/// cache serves the background from the second job on (within an hour)
/// instead of reading it again.
pub fn system_prompt(ws: &Workspace, analysis: bool, gates: bool, files: &[String]) -> String {
    let mut s = instructions(ws);
    s.push_str("\n\n");
    s.push_str(CONTRACT_COMMON);
    if let Some(b) = contract_bases(&ws.bases()) {
        s.push_str("\n\n");
        s.push_str(&b);
    }
    if gates {
        s.push('\n');
        s.push_str(CONTRACT_GATES);
    }
    s.push_str("\n\n");
    s.push_str(if analysis { CONTRACT_ANALYSIS } else { CONTRACT_BRIEF });
    if let Some(ev) = evidence_rules(ws) {
        s.push_str("\n\n## Evidence rules\n\n");
        s.push_str(&ev);
    }
    s.push_str("\n\n## My background\n\nEvery file is complete. Use it; it is the evidence for every claim.\n\n");
    for rel in files {
        let Some(text) = crate::context::read(ws, rel) else { continue };
        s.push_str(&format!("<document path=\"{rel}\">\n{}\n</document>\n\n", text.trim_end()));
    }
    for base in ws.bases() {
        if let Some(p) = printed_lines(ws, &base) {
            s.push_str(&p);
        }
    }
    s
}

/// The job-specific part: the page text.
pub fn user_message(job: &Job, posting: &str) -> String {
    let mut m = String::new();
    m.push_str(&format!(
        "<job_page source=\"{}\">\n{}\n</job_page>\n\nThe job page above is the full text of the page (navigation and footers included). \
         [tailor] my resume for the job it describes.",
        job.source.replace('"', "'"),
        posting.trim()
    ));
    m
}

/// How a base resume's PDF actually prints, so Claude can size rewrites
/// against real line widths (the 116 / 47 rule is about printed lines,
/// which are hard to judge from LaTeX source).
fn printed_lines(ws: &Workspace, base: &str) -> Option<String> {
    let pdf = base.strip_suffix(".tex").map(|b| format!("{b}.pdf"))?;
    let groups = crate::linecheck::printed(&ws.root.join(&pdf)).ok()?;
    if groups.is_empty() {
        return None;
    }
    let mut s = format!(
        "<printed_lines source=\"{pdf}\">\nHow each bullet and Skills entry of {} prints today: the character count of each printed line, then the text. A printed line holds at most 116 characters; a wrapped entry's last line needs at least 47.\n",
        base_name(base)
    );
    for g in groups {
        let lens: Vec<String> = g.iter().map(|l| l.chars().count().to_string()).collect();
        s.push_str(&format!("[{}] {}\n", lens.join(", "), g.join(" ")));
    }
    s.push_str("</printed_lines>\n\n");
    Some(s)
}

/// A small follow-up after the compiled PDF broke a rule: the numbered
/// edits and the problems (the base resumes are already in the system
/// prompt). Only the edits that change come back.
pub fn fix_message(base: &str, edits: &[crate::edits::Edit], problems: &[String]) -> String {
    let numbered: Vec<Value> = edits
        .iter()
        .enumerate()
        .map(|(i, e)| json!({"index": i + 1, "op": e.op(), "section": e.section, "entry": e.entry, "original": e.original, "replacement": e.replacement}))
        .collect();
    format!(
        "These edits were applied to {}:\n```json\n{}\n```\n\n\
         The compiled resume has these problems:\n{}\n\n\
         Return only the edits that must change to fix them, each with its `index` (or index 0 for a new edit). \
         Reword as little as possible, keep the same targeting and keywords, and keep every claim true.",
        base_name(base),
        serde_json::to_string_pretty(&numbered).unwrap_or_default(),
        problems.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")
    )
}

fn edit_schema(with_index: bool) -> Value {
    let mut props = json!({
        "op": {"type": "string", "enum": ["replace", "delete", "insert_after"]},
        "section": {"type": "string", "description": "Skills, Experience, Projects, ..."},
        "entry": {"type": "string", "description": "Which job, project or skills line"},
        "original": {"type": "string", "description": "Exact text from the base resume"},
        "replacement": {"type": "string"},
        "rationale": {"type": "string", "description": "One short sentence: which requirement or keyword"},
        "keywords": {"type": "array", "items": {"type": "string"}}
    });
    let mut required = vec!["op", "section", "entry", "original", "replacement", "rationale"];
    if with_index {
        props["index"] = json!({"type": "integer", "description": "1-based edit number being replaced; 0 for a new edit"});
        required.insert(0, "index");
    }
    json!({"type": "object", "properties": props, "required": required})
}

/// JSON Schema for the answer (`claude --json-schema`). `bases`: the base
/// resumes to choose from (a `base` field is asked for when there are two).
pub fn schema(analysis: bool, bases: &[String]) -> Value {
    let mut props = json!({
        "company": {"type": "string", "description": "The hiring company's usual short name, e.g. TickPick"},
        "position": {"type": "string", "description": "The job title as posted, without location or requisition id"},
        "decision": {"type": "string", "enum": ["tailor", "stop"]},
        "stop_reason": {"type": "string"},
        "edits": {"type": "array", "items": edit_schema(false)}
    });
    let mut required = vec!["company", "position", "decision", "edits"];
    if bases.len() > 1 {
        let names: Vec<&str> = bases.iter().map(|b| base_name(b)).collect();
        props["base"] = json!({"type": "string", "enum": names, "description": "The base resume the edits apply to"});
        required.insert(0, "base");
    }
    if analysis {
        props["analysis"] = json!({"type": "string", "description": "Markdown: the full [tailor] analysis"});
        required.push("analysis");
    }
    json!({"type": "object", "properties": props, "required": required})
}

/// JSON Schema for a fix answer: only the changed edits.
pub fn fix_schema() -> Value {
    json!({"type": "object", "properties": {"edits": {"type": "array", "items": edit_schema(true)}}, "required": ["edits"]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_sections() {
        let t = "# T\n\n## Part 1: mine\n\nDo A.\n### 0. sub\nB\n\n## Part 2: x\n\nC\n\n## Part 3: ev\n\nD\n";
        assert_eq!(section(t, "## Part 1").unwrap(), "Do A.\n### 0. sub\nB");
        assert_eq!(section(t, "## Part 3").unwrap(), "D");
    }

    #[test]
    fn asks_for_a_base_only_when_there_are_two() {
        let one = vec!["resumes/base.tex".to_string()];
        let two = vec!["resumes/base.tex".to_string(), "resumes/base-data.tex".to_string()];
        assert!(schema(false, &one)["properties"].get("base").is_none());
        let s = schema(false, &two);
        assert_eq!(s["properties"]["base"]["enum"], json!(["base.tex", "base-data.tex"]));
        assert_eq!(s["required"][0], "base");
        assert!(contract_bases(&one).is_none());
        assert!(contract_bases(&two).unwrap().contains("`base-data.tex`"));
    }
}
