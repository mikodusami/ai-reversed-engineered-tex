//! One posting → one tailored, compiled, checked resume.
//!
//! 1. Ask Claude (your instructions, your files in full, the posting) for the
//!    edits only — the role analysis is optional and off by default. The
//!    answer streams, so each edit is shown as soon as it is written.
//! 2. Apply the edits to a copy of the base resume Claude chose (base.tex, or
//!    base-data.tex for data roles): replace, delete, insert_after.
//! 3. Compile with rtex (build files hidden), check one page and the line rule.
//! 4. If something fails, send a small follow-up with just the numbered
//!    edits and the problems; only the failing edits come back.
//! 5. Put the PDF in the job's `attachments/` as
//!    `company.position.owner.MM-DD-YYYY.pdf` (the .tex stays in
//!    `resumes/tailored/` under the same name) and record the run in the
//!    hidden `.tailor.json` the app reads.

use crate::claude::{self, Event, Options, Reply};
use crate::edits::{self, Edit};
use crate::linecheck::{self, Violation};
use crate::prompt;
use crate::workspace::{Job, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TailorOptions {
    /// Also write the role analysis (strategic analysis, mapping, keywords
    /// table, fit, positioning). Off by default: slower, and only for you.
    pub analysis: bool,
}

/// Progress for the UI: JSON objects with a `kind`.
/// `{"kind":"status","text":…}` · `{"kind":"edit","index":n,"edit":{…}}` (streamed, 0-based)
pub type Progress<'a> = &'a mut dyn FnMut(&Value);

#[derive(Debug, Clone, Serialize)]
pub struct CallStat {
    pub purpose: String,
    #[serde(flatten)]
    pub reply: Reply,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    /// As Claude read them from the page (used for the file name).
    pub company: String,
    pub position: String,
    pub decision: String,
    pub stop_reason: String,
    /// The base resume the edits apply to (`resumes/base.tex` or `resumes/base-data.tex`).
    pub base: String,
    pub analysis: String,
    pub edits: Vec<Edit>,
    pub unapplied: Vec<String>,
    pub tex: Option<PathBuf>,
    pub pdf: Option<PathBuf>,
    pub attachment: Option<PathBuf>,
    pub pages: Option<u32>,
    pub violations: Vec<Violation>,
    pub rounds: u32,
    pub cost_usd: f64,
    pub seconds: f64,
    pub model: String,
    pub files_sent: Vec<String>,
    /// Why fixing stopped early (e.g. a usage limit), if it did.
    pub note: String,
    /// New Skills terms not found anywhere in the resumes or background sent.
    pub check_claims: Vec<String>,
    /// Each Claude call: purpose, seconds, tokens, cost.
    pub calls: Vec<CallStat>,
    /// Checks and edits as Markdown, for the app.
    pub summary: String,
}

impl Outcome {
    /// Ready to upload: compiled, one page, no new line-rule failures, every edit applied.
    pub fn ready(&self) -> bool {
        self.decision == "tailor"
            && self.pdf.is_some()
            && self.pages == Some(1)
            && self.unapplied.is_empty()
            && self.violations.iter().all(|v| v.in_base)
    }
}

fn status(progress: &mut dyn FnMut(&Value), text: &str) {
    progress(&json!({"kind": "status", "text": text}));
}

pub fn tailor(ws: &Workspace, job: &Job, topts: &TailorOptions, progress: Progress) -> Result<Outcome, String> {
    let t0 = Instant::now();
    let settings = ws.settings();
    let analysis = topts.analysis || settings.analysis;
    let opts = Options {
        model: (!settings.model.is_empty()).then(|| settings.model.clone()),
        effort: (!settings.effort.is_empty()).then(|| settings.effort.clone()),
    };
    let bases = ws.bases();
    refresh_base_pdfs(ws, &bases, progress);
    let posting = ws.posting_text(job).map_err(|e| format!("posting.md: {e}"))?;
    let files = crate::context::selected(ws, job);
    if !ws.tailoring_md().is_file() {
        return Err(format!("no tailoring instructions: put them in {} (or tailoring.md in the job folder)", ws.tailoring_md().display()));
    }
    let system = prompt::system_prompt(ws, analysis, settings.hard_gates, &files);
    let first = prompt::user_message(job, &posting);
    let mut calls: Vec<CallStat> = Vec::new();

    status(progress, &format!("Asking Claude ({} context files{})…", files.len(), if analysis { ", with analysis" } else { "" }));
    let reply = {
        let mut on = |e: Event| match e {
            Event::Thinking => status(progress, "Claude is thinking…"),
            Event::Writing => status(progress, "Claude is writing the edits…"),
            Event::Edit(i, v) => {
                progress(&json!({"kind": "edit", "index": i, "edit": v}));
                status(progress, &format!("Edit {} written…", i + 1));
            }
        };
        claude::ask(&opts, &system, &first, &prompt::schema(analysis, &bases), &mut on)?
    };
    let answer = reply.value.clone();
    let model = reply.model.clone();
    calls.push(CallStat { purpose: "tailor".into(), reply });

    let chosen = answer["base"].as_str().unwrap_or("");
    let base_rel = bases.iter().find(|b| prompt::base_name(b) == chosen).unwrap_or(&bases[0]).clone();
    let base = std::fs::read_to_string(ws.root.join(&base_rel)).map_err(|e| format!("{base_rel}: {e}"))?;
    let base_pdf = ws.root.join(base_rel.replace(".tex", ".pdf"));
    let decision = answer["decision"].as_str().unwrap_or("tailor").to_string();
    let mut edits: Vec<Edit> = serde_json::from_value(answer["edits"].clone()).unwrap_or_default();
    let company = answer["company"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(&job.company).trim().to_string();
    let position = answer["position"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(&job.title).trim().to_string();
    let mut out = Outcome {
        company: company.clone(),
        position: position.clone(),
        decision: decision.clone(),
        stop_reason: answer["stop_reason"].as_str().unwrap_or("").into(),
        base: base_rel.clone(),
        analysis: answer["analysis"].as_str().unwrap_or("").into(),
        edits: edits.clone(),
        unapplied: vec![],
        tex: None,
        pdf: None,
        attachment: None,
        pages: None,
        violations: vec![],
        rounds: 1,
        cost_usd: 0.0,
        seconds: 0.0,
        model,
        files_sent: files.clone(),
        note: String::new(),
        check_claims: vec![],
        calls: vec![],
        summary: String::new(),
    };
    if decision == "stop" {
        status(progress, "Claude stopped: a hard requirement is not met.");
        return finish(ws, job, out, calls, t0);
    }

    let name = resume_name(&company, &position, &settings.owner);
    let tex_path = ws.tailored_dir().join(format!("{name}.tex"));
    loop {
        status(progress, &format!("Applying {} edits to {} and compiling…", edits.len(), prompt::base_name(&base_rel)));
        let applied = edits::apply(&base, &edits);
        std::fs::create_dir_all(ws.tailored_dir()).map_err(|e| e.to_string())?;
        std::fs::write(&tex_path, &applied.tex).map_err(|e| e.to_string())?;
        let (pdf, pages, build_error) = compile(&tex_path);
        let violations = match &pdf {
            Some(p) => linecheck::check(p, Some(&base_pdf)).unwrap_or_default(),
            None => vec![],
        };
        let mut problems: Vec<String> = applied
            .problems
            .iter()
            .map(|(i, why)| format!("edit {} ({} / {}): {why}", i + 1, edits[*i].section, edits[*i].entry))
            .collect();
        if let Some(err) = &build_error {
            problems.push(format!("the resume did not compile: {err}"));
        }
        if let Some(p) = pages.filter(|p| *p != 1) {
            problems.push(format!(
                "the resume is {p} pages; it must be exactly one page (shorten or remove the least relevant content)"
            ));
        }
        for v in violations.iter().filter(|v| !v.in_base) {
            let owner = owner_of(&v.line, &edits).map(|i| format!(" [edit {}]", i + 1)).unwrap_or_default();
            problems.push(format!("{}{owner}", v.describe()));
        }

        out.edits = edits.clone();
        out.unapplied = applied.problems.iter().map(|(_, w)| w.clone()).collect();
        out.tex = Some(tex_path.clone());
        out.pages = pages;
        out.violations = violations;
        if problems.is_empty() || out.rounds > settings.fix_rounds || !out.note.is_empty() {
            status(progress, if problems.is_empty() { "All checks pass." } else { "Stopped fixing; some checks still fail." });
            out.attachment = pdf.as_ref().and_then(|p| attach(job, p, &format!("{name}.pdf")).ok());
            out.pdf = pdf;
            // Evidence = everything Claude was given (resumes and background).
            let verified: Vec<String> = files.iter().filter_map(|p| crate::context::read(ws, p)).collect();
            out.check_claims = crate::claims::unverified_skills(&base, &applied.tex, &verified);
            return finish(ws, job, out, calls, t0);
        }

        status(progress, &format!("Fixing {} problem(s)…", problems.len()));
        let msg = prompt::fix_message(&base_rel, &edits, &problems);
        let mut on = |e: Event| {
            if let Event::Edit(_, v) = e {
                let i = v["index"].as_u64().unwrap_or(0);
                status(progress, &format!("Rewrote edit {i}…"));
            }
        };
        let fix_opts = Options { effort: Some(settings.fix_effort.clone()), ..opts.clone() };
        match claude::ask(&fix_opts, &system, &msg, &prompt::fix_schema(), &mut on) {
            Ok(reply) => {
                let changes = reply.value["edits"].clone();
                calls.push(CallStat { purpose: "fix".into(), reply });
                merge_fixes(&mut edits, &changes);
                out.rounds += 1;
            }
            // Keep what we have (it compiled) rather than losing the run.
            Err(e) => {
                status(progress, &format!("Fix request failed: {e}. Keeping the last version."));
                out.note = format!("fixing stopped early: {e}");
            }
        }
    }
}

/// Recompile a base resume whose PDF is missing or older than its .tex: the
/// line check and the printed line widths sent to Claude come from the PDF.
fn refresh_base_pdfs(ws: &Workspace, bases: &[String], progress: &mut dyn FnMut(&Value)) {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    for b in bases {
        let tex = ws.root.join(b);
        let pdf = tex.with_extension("pdf");
        if mtime(&pdf) >= mtime(&tex) {
            continue;
        }
        status(progress, &format!("{} changed; recompiling it…", prompt::base_name(b)));
        let (_, _, err) = compile(&tex);
        if let Some(e) = err {
            status(progress, &format!("{} did not compile: {e}", prompt::base_name(b)));
        }
    }
}

fn finish(ws: &Workspace, job: &Job, mut out: Outcome, calls: Vec<CallStat>, t0: Instant) -> Result<Outcome, String> {
    out.cost_usd = calls.iter().map(|c| c.reply.cost_usd).sum();
    out.seconds = t0.elapsed().as_secs_f64();
    out.calls = calls;
    write_record(ws, job, &mut out)?;
    Ok(out)
}

/// Fold a fix answer into the edit list: `index` n replaces edit n
/// (1-based); index 0 (or out of range) appends a new edit.
pub fn merge_fixes(edits: &mut Vec<Edit>, changes: &Value) {
    let Some(list) = changes.as_array() else { return };
    for c in list {
        let Ok(e) = serde_json::from_value::<Edit>(c.clone()) else { continue };
        let i = c["index"].as_u64().unwrap_or(0) as usize;
        if i >= 1 && i <= edits.len() {
            edits[i - 1] = e;
        } else {
            edits.push(e);
        }
    }
}

/// Which edit wrote a printed line: the one whose replacement contains the
/// most of the line's words.
fn owner_of(line: &str, edits: &[Edit]) -> Option<usize> {
    let words: Vec<String> = line.split_whitespace().map(|w| w.to_lowercase()).filter(|w| w.len() > 2).collect();
    if words.is_empty() {
        return None;
    }
    edits
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let r = plain(&e.replacement).to_lowercase();
            (i, words.iter().filter(|w| r.contains(w.as_str())).count())
        })
        .filter(|(_, n)| *n * 2 >= words.len())
        .max_by_key(|(_, n)| *n)
        .map(|(i, _)| i)
}

/// LaTeX → roughly the printed text (for matching lines to edits).
fn plain(tex: &str) -> String {
    let re = regex::Regex::new(r"\\(newline|item|textbf|textit|emph|href)\b").unwrap();
    re.replace_all(tex, " ")
        .replace(['{', '}'], "")
        .replace("\\%", "%")
        .replace("\\&", "&")
        .replace("\\$", "$")
        .replace("\\\\", " ")
}

/// Compile with rtex-core; build files are hidden so `resumes/tailored/`
/// only holds .tex and .pdf. Returns (pdf, pages, error).
fn compile(tex: &Path) -> (Option<PathBuf>, Option<u32>, Option<String>) {
    let opts = rtex_core::BuildOptions { after_build: rtex_core::clean::AfterBuild::Hide, ..Default::default() };
    let mut sink = |_: &str| {};
    let report = rtex_core::build(tex, &opts, &mut sink, &AtomicBool::new(false));
    let error = report.error.clone().or_else(|| {
        report.log.as_ref().and_then(|l| l.errors().next().map(|e| format!("{} (line {})", e.message, e.line.unwrap_or(0))))
    });
    (report.pdf.clone(), report.pages, error)
}

/// Copy the PDF into the job's `attachments/` under the upload name.
fn attach(job: &Job, pdf: &Path, name: &str) -> std::io::Result<PathBuf> {
    let dir = job.dir.join("attachments");
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(name);
    std::fs::copy(pdf, &dest)?;
    Ok(dest)
}

/// `tickpick.software-engineer.odusami.10-27-2026`: company and position
/// kebab-cased, your name tag, today's date as MM-DD-YYYY.
pub fn resume_name(company: &str, position: &str, owner: &str) -> String {
    let (y, m, d) = crate::today();
    let company = crate::workspace::slugify(company);
    let position = crate::workspace::slugify(position);
    format!(
        "{}.{}.{owner}.{m:02}-{d:02}-{y}",
        if company.is_empty() { "company" } else { &company },
        if position.is_empty() { "position" } else { &position }
    )
}

fn rel(ws: &Workspace, p: &Path) -> String {
    p.strip_prefix(&ws.root).unwrap_or(p).display().to_string()
}

/// Markdown for one edit (also used by the app's live view).
pub fn edit_markdown(e: &Edit) -> String {
    let heading = format!("### {}: {}", e.section, e.entry);
    match e.op() {
        "delete" => format!("{heading} (removed)\nRemoved:\n```latex\n{}\n```\n{}\n", e.original.trim(), e.rationale.trim()),
        "insert_after" => format!("{heading} (added)\nAdded:\n```latex\n{}\n```\n{}\n", e.replacement.trim(), e.rationale.trim()),
        _ => format!(
            "{heading}\nOriginal:\n```latex\n{}\n```\nReplacement:\n```latex\n{}\n```\n{}\n",
            e.original.trim(),
            e.replacement.trim(),
            e.rationale.trim()
        ),
    }
}

/// Checks and edits as Markdown (shown by the app; not written as a file).
pub fn summary_markdown(ws: &Workspace, o: &Outcome) -> String {
    let mut r = format!("# {} - {}\n", or_dash(&o.company), or_dash(&o.position));
    if o.decision == "stop" {
        r.push_str(&format!("\n## Not tailored\n- {}\n", o.stop_reason));
        return r;
    }
    r.push_str("\n## Checks\n");
    if !o.base.is_empty() {
        r.push_str(&format!("- Base: {}\n", prompt::base_name(&o.base)));
    }
    if let Some(pdf) = &o.attachment.as_ref().or(o.pdf.as_ref()) {
        r.push_str(&format!("- Resume: {}\n", rel(ws, pdf)));
    }
    r.push_str(&format!("- Pages: {}\n", o.pages.map(|p| p.to_string()).unwrap_or_else(|| "did not compile".into())));
    let fails: Vec<&Violation> = o.violations.iter().filter(|v| !v.in_base).collect();
    if fails.is_empty() {
        r.push_str("- Line rule: ok\n");
    }
    for v in fails {
        r.push_str(&format!("- Line rule: {}\n", v.describe()));
    }
    for c in &o.check_claims {
        r.push_str(&format!("- Check this claim: Skills now lists `{c}`, which no resume or background file mentions\n"));
    }
    for u in &o.unapplied {
        r.push_str(&format!("- Not applied: {u}\n"));
    }
    if !o.note.is_empty() {
        r.push_str(&format!("- Note: {}\n", o.note));
    }
    if !o.edits.is_empty() {
        r.push_str(&format!("\n## Changes ({})\n", o.edits.len()));
        for e in &o.edits {
            r.push('\n');
            r.push_str(&edit_markdown(e));
        }
    }
    if !o.analysis.trim().is_empty() {
        r.push_str(&format!("\n## Analysis\n\n{}\n", demote(&o.analysis)));
    }
    let calls: Vec<String> = o
        .calls
        .iter()
        .map(|c| format!("{} {:.0} s / {} tokens out / {} cached in", c.purpose, c.reply.seconds, c.reply.output_tokens, c.reply.cached_input_tokens))
        .collect();
    r.push_str(&format!(
        "\n## Run\n- {} · {:.0} s total ({}) · ${:.2} list price · {} context files\n",
        or_dash(&o.model),
        o.seconds,
        calls.join(", "),
        o.cost_usd,
        o.files_sent.len()
    ));
    r
}

/// The hidden run record (`.tailor.json`) the app reads.
fn write_record(ws: &Workspace, job: &Job, o: &mut Outcome) -> Result<(), String> {
    o.summary = summary_markdown(ws, o);
    let path = job.dir.join(crate::workspace::RECORD_FILE);
    std::fs::write(path, serde_json::to_vec_pretty(o).unwrap()).map_err(|e| e.to_string())
}

/// Push the analysis's headings one level down under `## Analysis`.
fn demote(md: &str) -> String {
    md.lines().map(|l| if l.starts_with('#') { format!("##{l}") } else { l.to_string() }).collect::<Vec<_>>().join("\n")
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() {
        "—"
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(rep: &str) -> Edit {
        Edit { original: "x".into(), replacement: rep.into(), ..Default::default() }
    }

    #[test]
    fn merges_fixes_by_index() {
        let mut edits = vec![e("one"), e("two")];
        merge_fixes(
            &mut edits,
            &json!([{"index": 2, "op": "replace", "section": "S", "entry": "E", "original": "x", "replacement": "TWO", "rationale": "r"},
                    {"index": 0, "op": "delete", "section": "P", "entry": "Old", "original": "y", "replacement": "", "rationale": "r"}]),
        );
        assert_eq!(edits.len(), 3);
        assert_eq!(edits[1].replacement, "TWO");
        assert_eq!(edits[2].op(), "delete");
    }

    #[test]
    fn finds_the_edit_behind_a_line() {
        let edits = vec![e("\\item Built a React form for incidents"), e("\\textbf{Technologies:} React, Next.js, Node.js, PostgreSQL, MySQL")];
        assert_eq!(owner_of("React, Next.js, Node.js, PostgreSQL, MySQL, pgvector", &edits), Some(1));
        assert_eq!(owner_of("Completely unrelated words here", &edits), None);
    }
}
