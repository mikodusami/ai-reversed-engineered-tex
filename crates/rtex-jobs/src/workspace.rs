//! The job-search folder.
//!
//! ```text
//! <root>/resumes/base.tex, base.pdf, detailed-resume.tex   the resume and its inventory
//! <root>/background/…                                      evidence, always sent to Claude
//! <root>/auto-apply/tailoring.md                           your tailoring instructions
//! <root>/jobs/<stage>/<job>/page.txt                       the job page's text
//! <root>/jobs/<stage>/<job>/attachments/<resume>.pdf       the tailored resume to upload
//! <root>/jobs/<stage>/<job>/.tailor.json                   what was changed and checked (for the app)
//! ```
//!
//! Older job folders with a `posting.md` (YAML front matter + text) are read too.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// Pipeline stages, in board order.
pub const STAGES: &[&str] = &["inbox", "apply-now", "hold-referral", "applied", "do-not-apply"];

/// The job page text file in a job folder.
pub const PAGE_FILE: &str = "page.txt";
/// The run record the app reads (hidden: not part of the job's files).
pub const RECORD_FILE: &str = ".tailor.json";

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Name tag in resume file names (`company.position.<owner>.date.pdf`);
    /// default: the last name on base.tex.
    pub owner: String,
    /// Model passed to `claude --model` (empty: Claude Code's default).
    pub model: String,
    /// `claude --effort` level (empty: default).
    pub effort: String,
    /// Line-rule fix rounds after the first answer.
    pub fix_rounds: u32,
    /// Always write the role analysis (otherwise only when asked per run).
    pub analysis: bool,
    /// `claude --effort` for fix rounds (mechanical rewording); default "low".
    pub fix_effort: String,
    /// Let Claude refuse jobs that fail a hard requirement (clearance, salary
    /// floor, contract roles…). Off: every job gets a resume.
    pub hard_gates: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub slug: String,
    pub stage: String,
    pub dir: PathBuf,
    /// Position and company, once known (from the tailoring run, or an old posting.md).
    pub title: String,
    pub company: String,
    pub location: String,
    pub pay: String,
    /// The page the text came from.
    pub source: String,
    pub apply_url: String,
    pub captured_at: String,
    /// `page.txt` (or `posting.md` for older folders).
    pub page_file: PathBuf,
    pub has_review: bool,
    /// The tailored PDF in `attachments/`, when one has been built.
    pub resume_pdf: Option<PathBuf>,
}

/// A captured job page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Posting {
    /// Used for the folder name when known (page title, or what you typed).
    pub title: String,
    pub company: String,
    pub location: String,
    pub pay: String,
    pub source: String,
    pub apply_url: String,
    pub requisition: String,
    /// The page's text.
    pub body: String,
}

impl Workspace {
    /// `$RTEX_JOBS_ROOT`, else `~/Projects/career-job-search`.
    pub fn default_root() -> PathBuf {
        if let Some(p) = std::env::var_os("RTEX_JOBS_ROOT") {
            return PathBuf::from(p);
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
        home.join("Projects/career-job-search")
    }

    pub fn open(root: &Path) -> io::Result<Workspace> {
        let ws = Workspace { root: root.to_path_buf() };
        if !ws.base_tex().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} has no resumes/base.tex", root.display()),
            ));
        }
        for s in STAGES {
            std::fs::create_dir_all(ws.root.join("jobs").join(s))?;
        }
        Ok(ws)
    }

    pub fn base_tex(&self) -> PathBuf {
        self.root.join("resumes/base.tex")
    }
    pub fn base_pdf(&self) -> PathBuf {
        self.root.join("resumes/base.pdf")
    }
    pub fn tailored_dir(&self) -> PathBuf {
        self.root.join("resumes/tailored")
    }
    /// Your tailoring instructions: `<root>/tailoring.md`, else
    /// `<root>/auto-apply/tailoring.md`, else RTeX's own copy in
    /// `~/Library/Application Support/RTeX/tailoring.md`.
    pub fn tailoring_md(&self) -> PathBuf {
        let candidates = [self.root.join("tailoring.md"), self.root.join("auto-apply/tailoring.md"), app_support().join("tailoring.md")];
        candidates.iter().find(|p| p.is_file()).cloned().unwrap_or_else(|| candidates[2].clone())
    }
    pub fn settings_path(&self) -> PathBuf {
        self.root.join("rtex-jobs.json")
    }

    pub fn settings(&self) -> Settings {
        let mut s: Settings = std::fs::read(self.settings_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if s.owner.is_empty() {
            s.owner = std::fs::read_to_string(self.base_tex())
                .ok()
                .and_then(|t| owner_from_tex(&t))
                .unwrap_or_else(|| "resume".into());
        }
        if s.fix_rounds == 0 {
            s.fix_rounds = 2;
        }
        if s.fix_effort.is_empty() {
            s.fix_effort = "low".into();
        }
        s
    }

    pub fn jobs(&self) -> Vec<Job> {
        let mut out = Vec::new();
        for stage in STAGES {
            let Ok(rd) = std::fs::read_dir(self.root.join("jobs").join(stage)) else { continue };
            let mut dirs: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.join(PAGE_FILE).is_file() || p.join("posting.md").is_file())
                .collect();
            dirs.sort();
            for d in dirs {
                if let Some(j) = self.load(&d, stage) {
                    out.push(j);
                }
            }
        }
        out
    }

    pub fn find(&self, slug: &str) -> Option<Job> {
        self.jobs().into_iter().find(|j| j.slug == slug)
    }

    fn load(&self, dir: &Path, stage: &str) -> Option<Job> {
        let slug = dir.file_name()?.to_string_lossy().to_string();
        let page = dir.join(PAGE_FILE);
        let (page_file, mut fm) = if page.is_file() {
            let text = std::fs::read_to_string(&page).unwrap_or_default();
            (page, page_header(&text))
        } else {
            let p = dir.join("posting.md");
            let text = std::fs::read_to_string(&p).ok()?;
            (p, front_matter(&text).0)
        };
        // The tailoring run knows the company and position.
        if let Some(rec) = std::fs::read(dir.join(RECORD_FILE)).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
            for (k, field) in [("company", "company"), ("position", "title")] {
                if let Some(v) = rec[k].as_str().filter(|v| !v.is_empty()) {
                    fm.insert(field.into(), v.into());
                }
            }
        }
        let get = |k: &str| fm.get(k).cloned().unwrap_or_default();
        let resume_pdf = std::fs::read_dir(dir.join("attachments")).ok().and_then(|rd| {
            let mut pdfs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "pdf")).collect();
            // Newest first: the latest tailoring wins.
            pdfs.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).and_then(|m| m.modified()).ok()));
            pdfs.into_iter().next()
        });
        let source = get("source");
        Some(Job {
            title: if get("title").is_empty() { humanize(&slug) } else { get("title") },
            company: get("company"),
            location: get("location"),
            pay: get("pay"),
            apply_url: if get("apply_url").is_empty() { source.clone() } else { get("apply_url") },
            source,
            captured_at: get("captured_at"),
            has_review: dir.join(RECORD_FILE).is_file() || dir.join("review.md").is_file(),
            resume_pdf,
            page_file,
            slug,
            stage: stage.into(),
            dir: dir.to_path_buf(),
        })
    }

    /// The job page text (page.txt without its header lines, or posting.md).
    pub fn posting_text(&self, job: &Job) -> io::Result<String> {
        let text = std::fs::read_to_string(&job.page_file)?;
        if job.page_file.file_name().is_some_and(|n| n == PAGE_FILE) {
            return Ok(strip_page_header(&text).to_string());
        }
        Ok(text)
    }

    /// Save a captured page as `jobs/inbox/<job>/page.txt` (and an empty
    /// `attachments/`). Returns the job.
    pub fn create(&self, p: &Posting) -> io::Result<Job> {
        let hint = [p.company.as_str(), p.title.as_str()].join(" ");
        let mut base = slugify(&hint);
        if base.is_empty() {
            base = slugify(&url_hint(&p.source));
        }
        if base.is_empty() {
            base = format!("job-{}", slugify(&crate::now_iso()));
        }
        let mut slug = base.clone();
        let mut n = 2;
        while self.find(&slug).is_some() || self.root.join("jobs/inbox").join(&slug).exists() {
            slug = format!("{base}-{n}");
            n += 1;
        }
        let dir = self.root.join("jobs/inbox").join(&slug);
        std::fs::create_dir_all(dir.join("attachments"))?;
        let mut header = String::new();
        if !p.source.is_empty() {
            header.push_str(&format!("Source: {}\n", p.source));
        }
        header.push_str(&format!("Captured: {}\n\n", crate::now_iso()));
        std::fs::write(dir.join(PAGE_FILE), format!("{header}{}\n", p.body.trim()))?;
        self.load(&dir, "inbox").ok_or_else(|| io::Error::other("could not read back the new job"))
    }

    /// Move a job folder to another stage.
    pub fn move_job(&self, job: &Job, stage: &str) -> io::Result<Job> {
        if !STAGES.contains(&stage) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("unknown stage {stage}")));
        }
        if job.stage == stage {
            return Ok(job.clone());
        }
        let dest = self.root.join("jobs").join(stage).join(&job.slug);
        std::fs::rename(&job.dir, &dest)?;
        self.load(&dest, stage).ok_or_else(|| io::Error::other("moved job could not be read"))
    }
}

/// `~/Library/Application Support/RTeX` (RTeX's own settings).
pub fn app_support() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
    home.join("Library/Application Support/RTeX")
}

/// `Source:` / `Captured:` lines at the top of page.txt.
fn page_header(text: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for line in text.lines().take(3) {
        if let Some(v) = line.strip_prefix("Source: ") {
            m.insert("source".into(), v.trim().into());
        } else if let Some(v) = line.strip_prefix("Captured: ") {
            m.insert("captured_at".into(), v.trim().into());
        }
    }
    m
}

fn strip_page_header(text: &str) -> &str {
    let mut rest = text;
    for _ in 0..2 {
        if rest.starts_with("Source: ") || rest.starts_with("Captured: ") {
            rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or("");
        }
    }
    rest.trim_start_matches('\n')
}

/// A name hint from a URL (`https://apply.workable.com/tickpick/j/5840ECEB50/` → `workable tickpick 5840ECEB50`).
fn url_hint(url: &str) -> String {
    let path = url.split("://").nth(1).unwrap_or(url);
    let mut parts = path.split(['/', '?', '#']).filter(|p| !p.is_empty());
    let host = parts.next().unwrap_or("");
    let host_name = host.split('.').rev().nth(1).unwrap_or(host);
    let words: Vec<&str> = parts.filter(|p| p.chars().any(|c| c.is_ascii_alphabetic()) && p.len() > 2).take(2).collect();
    format!("{host_name} {}", words.join(" "))
}

/// `tickpick-software-engineer` → `Tickpick software engineer`.
fn humanize(slug: &str) -> String {
    let s = slug.replace('-', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Last name from `\Huge First Last` (or `\name{…}`) in base.tex, lowercased.
pub fn owner_from_tex(tex: &str) -> Option<String> {
    let after = tex.find("\\Huge").map(|i| &tex[i + 5..]).or_else(|| tex.find("\\name{").map(|i| &tex[i + 6..]))?;
    let name: String = after.chars().take_while(|c| *c != '}' && *c != '\n').collect();
    let last = name.split_whitespace().last()?;
    let s = slugify(last);
    (!s.is_empty()).then_some(s)
}

/// `---\nkey: "value"\n---\nbody` → (fields, body).
pub fn front_matter(text: &str) -> (BTreeMap<String, String>, &str) {
    let mut fm = BTreeMap::new();
    let Some(rest) = text.strip_prefix("---\n") else { return (fm, text) };
    let Some(end) = rest.find("\n---") else { return (fm, text) };
    for line in rest[..end].lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v);
            fm.insert(k.trim().to_string(), v.replace("\\\"", "\"").replace("\\\\", "\\"));
        }
    }
    let body = rest[end + 4..].trim_start_matches('\n');
    (fm, body)
}

/// Lowercase, ASCII letters and digits, single dashes.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    out.chars().take(90).collect::<String>().trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_front_matter() {
        let (fm, body) = front_matter("---\ntitle: \"Software \\\"Eng\\\"\"\ncompany: Acme\n---\n\nBody here\n");
        assert_eq!(fm["title"], "Software \"Eng\"");
        assert_eq!(fm["company"], "Acme");
        assert_eq!(body, "Body here\n");
    }

    #[test]
    fn slugs_and_owner() {
        assert_eq!(slugify("TickPick Software Engineer New York, NY"), "tickpick-software-engineer-new-york-ny");
        assert_eq!(owner_from_tex("\\centerline{\\Huge Michael-Andre Odusami}"), Some("odusami".into()));
        assert_eq!(slugify(&url_hint("https://apply.workable.com/tickpick/j/5840ECEB50/")), "workable-tickpick-5840eceb50");
    }

    #[test]
    fn create_list_move() {
        let root = std::env::temp_dir().join(format!("rtex-ws-{}", std::process::id()));
        std::fs::create_dir_all(root.join("resumes")).unwrap();
        std::fs::write(root.join("resumes/base.tex"), "\\documentclass{article}").unwrap();
        let ws = Workspace::open(&root).unwrap();
        let p = Posting { title: "Engineer".into(), company: "Acme".into(), source: "https://acme.com/jobs/1".into(), body: "Do things".into(), ..Default::default() };
        let j = ws.create(&p).unwrap();
        assert_eq!(j.slug, "acme-engineer");
        assert!(j.dir.join("attachments").is_dir());
        assert_eq!(j.source, "https://acme.com/jobs/1");
        assert_eq!(ws.posting_text(&j).unwrap().trim(), "Do things");
        let j2 = ws.create(&p).unwrap();
        assert_eq!(j2.slug, "acme-engineer-2");
        let moved = ws.move_job(&j, "apply-now").unwrap();
        assert_eq!(moved.stage, "apply-now");
        assert_eq!(ws.jobs().len(), 2);
        std::fs::remove_dir_all(root).ok();
    }
}
