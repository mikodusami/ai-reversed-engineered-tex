//! What Claude reads for every job: the resumes and everything in
//! `background/`, in full (that's what makes tailoring as good as the web
//! app). PDFs (the transcript, certificates) are sent as their text,
//! extracted once with pdftotext and cached. A job can narrow the set with a
//! `context.txt` (one path per line, relative to the workspace root).
//!
//! The documents always come first and in the same order, before anything
//! job-specific, so Claude's prompt cache reuses them across jobs (within an
//! hour): later jobs pay for the background once, not every time.

use crate::workspace::{Job, Workspace};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct ContextFile {
    /// Path relative to the workspace root.
    pub path: String,
    pub bytes: u64,
    /// Sent by default.
    pub default: bool,
    pub note: String,
}

/// Largest file sent as context.
const MAX_BYTES: u64 = 1_500_000;

/// Every file that can be sent: the resumes and everything in `background/`.
pub fn available(ws: &Workspace) -> Vec<ContextFile> {
    let mut out = Vec::new();
    for (rel, note) in [
        ("resumes/base.tex", "the base resume (always sent)"),
        ("resumes/base-data.tex", "the base resume for data roles (always sent)"),
        ("resumes/detailed-resume.tex", "full inventory of verified bullets"),
    ] {
        if let Ok(m) = std::fs::metadata(ws.root.join(rel)) {
            out.push(ContextFile { path: rel.into(), bytes: m.len(), default: true, note: note.into() });
        }
    }
    let mut files = Vec::new();
    walk(&ws.root.join("background"), &mut files, 0);
    files.sort();
    for p in files {
        let rel = p.strip_prefix(&ws.root).unwrap_or(&p).to_string_lossy().to_string();
        let lower = rel.to_ascii_lowercase();
        let is_pdf = lower.ends_with(".pdf");
        // For PDFs, the size of the text actually sent.
        let bytes = if is_pdf {
            read(ws, &rel).map(|t| t.len() as u64).unwrap_or(0)
        } else {
            std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0)
        };
        let (default, note) = if lower.contains("salesforce") || lower.contains("mock") {
            (false, "not evidence (contains invented stories)")
        } else if bytes > MAX_BYTES {
            (false, "too large to send")
        } else if is_pdf {
            (true, "PDF, sent as its text")
        } else {
            (true, "background")
        };
        out.push(ContextFile { path: rel, bytes, default, note: note.into() });
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, out, depth + 1);
        } else if is_pdf(&p) || is_text(&p) {
            out.push(p);
        }
    }
}

fn is_pdf(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// Text files only (Markdown, LaTeX, plain text, extension-less notes).
fn is_text(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "zip" | "docx" | "pptx" | "key" | "gif" | "mov" | "mp4") {
        return false;
    }
    let Ok(bytes) = std::fs::read(p) else { return false };
    let head = &bytes[..bytes.len().min(4096)];
    // Valid UTF-8, allowing a multi-byte character cut at the 4 KB boundary.
    !head.contains(&0) && std::str::from_utf8(head).map_or_else(|e| e.error_len().is_none(), |_| true)
}

/// A context file's text: the file itself, or a PDF's extracted text
/// (cached by path and modification time).
pub fn read(ws: &Workspace, rel: &str) -> Option<String> {
    let path = ws.root.join(rel);
    if !is_pdf(&path) {
        return std::fs::read_to_string(&path).ok();
    }
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    let stamp = mtime.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let key = rtex_core::build::fnv(format!("{}:{stamp}", path.display()).as_bytes());
    let cache = rtex_core::build::cache_root().join("pdf-text").join(format!("{key:016x}.txt"));
    if let Ok(t) = std::fs::read_to_string(&cache) {
        return Some(t);
    }
    let bin = crate::linecheck::pdftotext()?;
    let out = Command::new(bin).arg("-layout").arg(&path).arg("-").output().ok()?;
    if !out.status.success() {
        return None;
    }
    // Collapse layout padding: it costs tokens and carries no meaning.
    let text: String = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&cache, &text);
    }
    Some(text)
}

/// The files to send for `job`: its `context.txt`, else the defaults.
/// The base resumes are always included, first.
pub fn selected(ws: &Workspace, job: &Job) -> Vec<String> {
    let rest: Vec<String> = match std::fs::read_to_string(job.dir.join("context.txt")) {
        Ok(t) => t.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect(),
        Err(_) => available(ws).into_iter().filter(|f| f.default).map(|f| f.path).collect(),
    };
    let mut sel = ws.bases();
    for p in rest {
        if !sel.contains(&p) {
            sel.push(p);
        }
    }
    sel.retain(|p| ws.root.join(p).is_file());
    sel
}

pub fn save_selection(job: &Job, paths: &[String]) -> std::io::Result<()> {
    let mut text = String::from("# Files sent to Claude when tailoring this job (relative to the workspace).\n");
    for p in paths {
        text.push_str(p);
        text.push('\n');
    }
    std::fs::write(job.dir.join("context.txt"), text)
}
