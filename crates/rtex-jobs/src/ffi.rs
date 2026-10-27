//! C ABI for the Jobs window (see app/include/rtex.h). Same conventions as
//! rtex-core's: UTF-8 strings in, JSON strings out (free them with
//! `rtex_string_free`), `{"error": ...}` on failure. The slow calls
//! (`rtex_jobs_add_url`, `rtex_jobs_tailor`) block: call them off the main thread.
//!
//! # Safety
//! Every pointer argument must be NULL or a valid NUL-terminated string, and
//! `ctx` must stay valid until `rtex_jobs_tailor` returns.
#![allow(clippy::missing_safety_doc)]

use crate::workspace::{Posting, Workspace};
use crate::{capture, context, tailor};
use serde_json::json;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;

fn to_c(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "")).unwrap().into_raw()
}

unsafe fn arg<'a>(p: *const c_char) -> &'a str {
    if p.is_null() {
        return "";
    }
    CStr::from_ptr(p).to_str().unwrap_or("")
}

fn ok<T: serde::Serialize>(v: &T) -> *mut c_char {
    to_c(serde_json::to_string(v).unwrap_or_else(|e| json!({ "error": e.to_string() }).to_string()))
}

fn err(msg: impl std::fmt::Display) -> *mut c_char {
    to_c(json!({ "error": msg.to_string() }).to_string())
}

unsafe fn workspace(root: *const c_char) -> Result<Workspace, String> {
    let r = arg(root);
    let path = if r.is_empty() { Workspace::default_root() } else { Path::new(r).to_path_buf() };
    Workspace::open(&path).map_err(|e| e.to_string())
}

/// The default job-search folder.
#[no_mangle]
pub extern "C" fn rtex_jobs_default_root() -> *mut c_char {
    to_c(Workspace::default_root().display().to_string())
}

/// `{"root": ..., "stages": [...], "jobs": [Job...]}`.
#[no_mangle]
pub unsafe extern "C" fn rtex_jobs_list(root: *const c_char) -> *mut c_char {
    match workspace(root) {
        Ok(ws) => ok(&json!({ "root": ws.root, "stages": crate::workspace::STAGES, "jobs": ws.jobs() })),
        Err(e) => err(e),
    }
}

/// Fetch a posting URL and save it in the inbox (blocking). Returns the Job.
#[no_mangle]
pub unsafe extern "C" fn rtex_jobs_add_url(root: *const c_char, url: *const c_char) -> *mut c_char {
    let ws = match workspace(root) {
        Ok(w) => w,
        Err(e) => return err(e),
    };
    let url = arg(url).trim();
    match capture::capture_url(url) {
        Ok(p) => match ws.create(&p) {
            Ok(j) => ok(&j),
            Err(e) => err(e),
        },
        Err(e) => err(e),
    }
}

/// Save HTML from a page you have open (the browser extension). Returns the Job.
#[no_mangle]
pub unsafe extern "C" fn rtex_jobs_add_html(root: *const c_char, html: *const c_char, url: *const c_char) -> *mut c_char {
    let ws = match workspace(root) {
        Ok(w) => w,
        Err(e) => return err(e),
    };
    match capture::from_html(arg(html), arg(url)) {
        Ok(p) => match ws.create(&p) {
            Ok(j) => ok(&j),
            Err(e) => err(e),
        },
        Err(e) => err(e),
    }
}

/// Save pasted text. `fields_json`: `{title, company, location, url}`.
#[no_mangle]
pub unsafe extern "C" fn rtex_jobs_add_text(root: *const c_char, text: *const c_char, fields_json: *const c_char) -> *mut c_char {
    let ws = match workspace(root) {
        Ok(w) => w,
        Err(e) => return err(e),
    };
    let f: Posting = serde_json::from_str(arg(fields_json)).unwrap_or_default();
    let p = capture::from_text(arg(text), &f.title, &f.company, &f.location, &f.source);
    match ws.create(&p) {
        Ok(j) => ok(&j),
        Err(e) => err(e),
    }
}

pub type ProgressFn = extern "C" fn(ctx: *mut c_void, event_json: *const c_char);

/// Tailor a job's resume (blocking). `options_json`: TailorOptions
/// (`{"analysis": false}`). `progress` gets JSON events as they happen:
/// `{"kind":"status","text":…}` and `{"kind":"edit","index":n,"edit":{…}}`.
/// With `move_when_ready`, a ready resume moves the job from inbox to apply-now.
/// Returns the Outcome plus `"ready"` and `"job"` (the job after any move).
#[no_mangle]
pub unsafe extern "C" fn rtex_jobs_tailor(
    root: *const c_char,
    slug: *const c_char,
    options_json: *const c_char,
    move_when_ready: bool,
    progress: Option<ProgressFn>,
    ctx: *mut c_void,
) -> *mut c_char {
    let ws = match workspace(root) {
        Ok(w) => w,
        Err(e) => return err(e),
    };
    let Some(job) = ws.find(arg(slug)) else { return err(format!("no job {}", arg(slug))) };
    let ctx_addr = ctx as usize;
    let topts: tailor::TailorOptions = serde_json::from_str(arg(options_json)).unwrap_or_default();
    let mut report = |ev: &serde_json::Value| {
        if let (Some(f), Ok(c)) = (progress, CString::new(ev.to_string().replace('\0', ""))) {
            f(ctx_addr as *mut c_void, c.as_ptr());
        }
    };
    match tailor::tailor(&ws, &job, &topts, &mut report) {
        Ok(out) => {
            let mut final_job = ws.find(&job.slug).unwrap_or(job.clone());
            if move_when_ready && out.ready() && final_job.stage == "inbox" {
                if let Ok(j) = ws.move_job(&final_job, "apply-now") {
                    final_job = j;
                }
            }
            let mut v = serde_json::to_value(&out).unwrap_or_default();
            v["ready"] = json!(out.ready());
            v["job"] = serde_json::to_value(&final_job).unwrap_or_default();
            ok(&v)
        }
        Err(e) => err(e),
    }
}

/// Move a job to another stage. Returns the Job.
#[no_mangle]
pub unsafe extern "C" fn rtex_jobs_move(root: *const c_char, slug: *const c_char, stage: *const c_char) -> *mut c_char {
    let ws = match workspace(root) {
        Ok(w) => w,
        Err(e) => return err(e),
    };
    let Some(job) = ws.find(arg(slug)) else { return err(format!("no job {}", arg(slug))) };
    match ws.move_job(&job, arg(stage)) {
        Ok(j) => ok(&j),
        Err(e) => err(e),
    }
}

/// `{"files": [ContextFile...], "selected": [path...]}` for a job.
#[no_mangle]
pub unsafe extern "C" fn rtex_jobs_context(root: *const c_char, slug: *const c_char) -> *mut c_char {
    let ws = match workspace(root) {
        Ok(w) => w,
        Err(e) => return err(e),
    };
    let Some(job) = ws.find(arg(slug)) else { return err(format!("no job {}", arg(slug))) };
    ok(&json!({ "files": context::available(&ws), "selected": context::selected(&ws, &job) }))
}

/// Save a job's context selection (`paths_json`: array of relative paths).
#[no_mangle]
pub unsafe extern "C" fn rtex_jobs_set_context(root: *const c_char, slug: *const c_char, paths_json: *const c_char) -> *mut c_char {
    let ws = match workspace(root) {
        Ok(w) => w,
        Err(e) => return err(e),
    };
    let Some(job) = ws.find(arg(slug)) else { return err(format!("no job {}", arg(slug))) };
    let paths: Vec<String> = serde_json::from_str(arg(paths_json)).unwrap_or_default();
    match context::save_selection(&job, &paths) {
        Ok(()) => ok(&json!({ "saved": paths.len() })),
        Err(e) => err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn call(p: *mut c_char) -> serde_json::Value {
        let s = CStr::from_ptr(p).to_str().unwrap().to_string();
        drop(CString::from_raw(p));
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn list_add_move_context() {
        let root = std::env::temp_dir().join(format!("rtex-jobs-ffi-{}", std::process::id()));
        std::fs::create_dir_all(root.join("resumes")).unwrap();
        std::fs::write(root.join("resumes/base.tex"), "\\documentclass{article}").unwrap();
        let r = CString::new(root.to_str().unwrap()).unwrap();
        unsafe {
            let text = CString::new("Build things with Rust.").unwrap();
            let fields = CString::new(r#"{"title":"Engineer","company":"Acme","location":"Remote"}"#).unwrap();
            let j = call(rtex_jobs_add_text(r.as_ptr(), text.as_ptr(), fields.as_ptr()));
            assert_eq!(j["slug"], "acme-engineer");
            let list = call(rtex_jobs_list(r.as_ptr()));
            assert_eq!(list["jobs"].as_array().unwrap().len(), 1);
            let slug = CString::new("acme-engineer").unwrap();
            let stage = CString::new("hold-referral").unwrap();
            assert_eq!(call(rtex_jobs_move(r.as_ptr(), slug.as_ptr(), stage.as_ptr()))["stage"], "hold-referral");
            let ctx = call(rtex_jobs_context(r.as_ptr(), slug.as_ptr()));
            assert_eq!(ctx["selected"][0], "resumes/base.tex");
            let bad = CString::new("nope").unwrap();
            assert!(call(rtex_jobs_move(r.as_ptr(), bad.as_ptr(), stage.as_ptr()))["error"].is_string());
        }
        std::fs::remove_dir_all(root).ok();
    }
}
