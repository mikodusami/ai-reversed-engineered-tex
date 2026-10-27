//! C ABI for the macOS app (see app/include/rtex.h).
//!
//! Strings in and out are NUL-terminated UTF-8. Every `char*` returned must
//! be released with `rtex_string_free`; span arrays with `rtex_spans_free`.
//! Structured results are JSON, which the app reads with NSJSONSerialization.
//!
//! # Safety
//! Every pointer argument must be NULL or valid for the documented use:
//! strings NUL-terminated, `text`/`len` a readable byte range, and freed
//! pointers ones previously returned by this library and not freed before.
#![allow(clippy::missing_safety_doc)]

use crate::build::{self, BuildOptions};
use crate::highlight::{self, Span};
use crate::synctex;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

fn to_c(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "")).unwrap().into_raw()
}

unsafe fn from_c<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

fn json<T: serde::Serialize>(v: &T) -> *mut c_char {
    to_c(serde_json::to_string(v).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")))
}

fn err(msg: impl std::fmt::Display) -> *mut c_char {
    to_c(serde_json::json!({ "error": msg.to_string() }).to_string())
}

#[no_mangle]
pub unsafe extern "C" fn rtex_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// Console output callback: `chunk` is valid only during the call.
pub type OutputFn = extern "C" fn(ctx: *mut c_void, chunk: *const c_char);

/// Cancellation token for `rtex_build`.
#[no_mangle]
pub extern "C" fn rtex_cancel_new() -> *mut AtomicBool {
    Box::into_raw(Box::new(AtomicBool::new(false)))
}

#[no_mangle]
pub unsafe extern "C" fn rtex_cancel_set(c: *mut AtomicBool) {
    if let Some(c) = c.as_ref() {
        c.store(true, Ordering::Relaxed);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rtex_cancel_free(c: *mut AtomicBool) {
    if !c.is_null() {
        drop(Box::from_raw(c));
    }
}

/// Typeset `path`. `options_json` is a BuildOptions object (may be NULL).
/// Blocks until done; call it off the main thread. Returns a BuildReport.
#[no_mangle]
pub unsafe extern "C" fn rtex_build(
    path: *const c_char,
    options_json: *const c_char,
    output: Option<OutputFn>,
    ctx: *mut c_void,
    cancel: *const AtomicBool,
) -> *mut c_char {
    let Some(path) = from_c(path) else { return err("path is NULL or not UTF-8") };
    let opts: BuildOptions = match from_c(options_json) {
        Some(j) if !j.is_empty() => match serde_json::from_str(j) {
            Ok(o) => o,
            Err(e) => return err(format!("bad options: {e}")),
        },
        _ => BuildOptions::default(),
    };
    let never = AtomicBool::new(false);
    let cancel = cancel.as_ref().unwrap_or(&never);
    let ctx_addr = ctx as usize;
    let mut sink = |s: &str| {
        if let Some(f) = output {
            if let Ok(c) = CString::new(s.replace('\0', "")) {
                f(ctx_addr as *mut c_void, c.as_ptr());
            }
        }
    };
    let report = build::build(Path::new(path), &opts, &mut sink, cancel);
    json(&report)
}

/// Forward search. Returns `{"page":N,"rects":[{x,y,width,height}],"matched_line":L}`
/// (PDF points, top-left origin) or `{"error":...}`.
#[no_mangle]
pub unsafe extern "C" fn rtex_synctex_forward(pdf: *const c_char, source: *const c_char, line: u32, page_hint: u32) -> *mut c_char {
    let (Some(pdf), Some(source)) = (from_c(pdf), from_c(source)) else { return err("NULL argument") };
    match synctex::cached(Path::new(pdf)) {
        Ok(s) => match s.forward(Path::new(source), line, (page_hint > 0).then_some(page_hint)) {
            Some(r) => json(&r),
            None => err("no match"),
        },
        Err(e) => err(e),
    }
}

/// Inverse search. Returns `{"file":path,"line":L}` or `{"error":...}`.
#[no_mangle]
pub unsafe extern "C" fn rtex_synctex_inverse(pdf: *const c_char, page: u32, x: f64, y: f64) -> *mut c_char {
    let Some(pdf) = from_c(pdf) else { return err("NULL argument") };
    match synctex::cached(Path::new(pdf)) {
        Ok(s) => match s.inverse(page, x, y) {
            Some(r) => json(&r),
            None => err("no match"),
        },
        Err(e) => err(e),
    }
}

/// Outline (sections, labels, `%:` marks) of a source text, as JSON array.
#[no_mangle]
pub unsafe extern "C" fn rtex_outline(text: *const c_char) -> *mut c_char {
    json(&crate::outline::outline(from_c(text).unwrap_or("")))
}

/// `% !TEX` directives of a source text, as JSON.
#[no_mangle]
pub unsafe extern "C" fn rtex_directives(text: *const c_char) -> *mut c_char {
    json(&crate::directives::Directives::parse(from_c(text).unwrap_or("")))
}

/// Parse a log file's text into a LogSummary (JSON).
#[no_mangle]
pub unsafe extern "C" fn rtex_parse_log(text: *const c_char) -> *mut c_char {
    json(&crate::log::parse(from_c(text).unwrap_or("")))
}

/// Highlight `len` bytes of UTF-8 text. Offsets in the returned spans are
/// UTF-16 units relative to the start of `text`.
#[no_mangle]
pub unsafe extern "C" fn rtex_highlight(text: *const u8, len: usize, count: *mut usize) -> *mut Span {
    let bytes = if text.is_null() { &[][..] } else { std::slice::from_raw_parts(text, len) };
    let s = String::from_utf8_lossy(bytes);
    let spans = highlight::highlight(&s).into_boxed_slice();
    if !count.is_null() {
        *count = spans.len();
    }
    Box::into_raw(spans) as *mut Span
}

#[no_mangle]
pub unsafe extern "C" fn rtex_spans_free(spans: *mut Span, count: usize) {
    if !spans.is_null() {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(spans, count)));
    }
}

/// Remove auxiliary files of a root file. Returns the removed paths (JSON).
#[no_mangle]
pub unsafe extern "C" fn rtex_clean(path: *const c_char, aggressive: bool) -> *mut c_char {
    let Some(p) = from_c(path) else { return err("NULL argument") };
    let root = crate::root::resolve_root(Path::new(p)).map(|(r, _)| r).unwrap_or_else(|_| p.into());
    match crate::clean::clean(&root, aggressive, &[]) {
        Ok(v) => json(&v),
        Err(e) => err(e),
    }
}

/// Resolve the root file for `path` (follows `% !TEX root`).
#[no_mangle]
pub unsafe extern "C" fn rtex_resolve_root(path: *const c_char) -> *mut c_char {
    let Some(p) = from_c(path) else { return err("NULL argument") };
    match crate::root::resolve_root(Path::new(p)) {
        Ok((r, d)) => json(&serde_json::json!({ "root": r, "directives": d })),
        Err(e) => err(e),
    }
}

/// Available engines (built-ins and `.engine` scripts), as JSON.
#[no_mangle]
pub extern "C" fn rtex_engines() -> *mut c_char {
    json(&crate::engine::list())
}
