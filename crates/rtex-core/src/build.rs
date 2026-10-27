//! The typeset pipeline (TeXShop: the `JobProcessing` category on
//! TSDocument; `doTypeset:`, `doBibtex:`, `handleMakeindexCommand:` ...).
//!
//! What rtex adds for speed:
//! * **Reruns only when needed.** It hashes `.aux`/`.toc`/`.lof`/`.out`/...
//!   (including `\include` sub-aux files) around each pass and reruns only
//!   if they changed or the log asks for it. An edit that moves no labels
//!   is a single pass.
//! * **BibTeX / Biber / makeindex only when their inputs change.** The keys
//!   (citations + .bib contents, .bcf, .idx) are kept in a per-document state
//!   file in the cache, so they survive between builds.
//! * **Optional precompiled preamble** (pdflatex + mylatexformat): the
//!   preamble is dumped into a format once and reused while unchanged.
//! * No Perl (latexmk) and no shell between rtex and the engine.

use crate::engine::{self, Engine, DEFAULT_BIN_DIR};
use crate::log::{self, LogSummary};
use crate::root::{read_lossy, resolve_root};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildOptions {
    /// Overrides `% !TEX program`; default `pdflatex`.
    pub engine: Option<String>,
    pub max_passes: u32,
    /// TeXShop behaviour: one engine run, no BibTeX/index/reruns.
    pub single_pass: bool,
    pub synctex: bool,
    pub shell_escape: bool,
    pub fast_preamble: bool,
    pub bibliography: bool,
    pub index: bool,
    pub bin_dir: PathBuf,
    pub extra_args: Vec<String>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            engine: None,
            max_passes: 5,
            single_pass: false,
            synctex: true,
            shell_escape: false,
            fast_preamble: false,
            bibliography: true,
            index: true,
            bin_dir: PathBuf::from(DEFAULT_BIN_DIR),
            extra_args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub tool: String,
    pub args: Vec<String>,
    pub reason: String,
    pub status: i32,
    pub millis: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildReport {
    pub root: PathBuf,
    pub engine: String,
    pub success: bool,
    pub cancelled: bool,
    pub pdf: Option<PathBuf>,
    pub pages: Option<u32>,
    pub passes: u32,
    pub steps: Vec<Step>,
    pub fast_preamble: Option<String>,
    pub log: Option<LogSummary>,
    pub error: Option<String>,
    pub total_millis: u64,
}

/// Output sink for console text (TeXShop's console window).
pub type Sink<'a> = &'a mut dyn FnMut(&str);

/// Typeset `file` (any file of the project; `% !TEX root` is followed).
pub fn build(file: &Path, opts: &BuildOptions, sink: Sink, cancel: &AtomicBool) -> BuildReport {
    let t0 = Instant::now();
    let mut report = BuildReport {
        root: file.to_path_buf(),
        engine: String::new(),
        success: false,
        cancelled: false,
        pdf: None,
        pages: None,
        passes: 0,
        steps: Vec::new(),
        fast_preamble: None,
        log: None,
        error: None,
        total_millis: 0,
    };
    if let Err(e) = build_inner(file, opts, sink, cancel, &mut report) {
        report.error = Some(e);
    }
    report.cancelled = cancel.load(Ordering::Relaxed);
    report.total_millis = t0.elapsed().as_millis() as u64;
    report
}

fn build_inner(
    file: &Path,
    opts: &BuildOptions,
    sink: Sink,
    cancel: &AtomicBool,
    report: &mut BuildReport,
) -> Result<(), String> {
    let (root, directives) = resolve_root(file).map_err(|e| e.to_string())?;
    report.root = root.clone();
    let dir = root.parent().ok_or("root has no directory")?.to_path_buf();
    let name = root.file_name().ok_or("bad root name")?.to_string_lossy().to_string();
    let stem = root.file_stem().unwrap().to_string_lossy().to_string();

    let engine_name = opts.engine.clone().or(directives.program.clone()).unwrap_or_else(|| "pdflatex".into());
    let engine = engine::resolve(&engine_name)?;
    report.engine = engine_name.clone();
    let env = tex_env(&opts.bin_dir);
    let tool_env = tool_env(&opts.bin_dir);
    let cache = doc_cache_dir(&root);
    let mut state = State::load(&cache);

    sink(&format!("rtex: typesetting {} with {}\n", root.display(), engine_name));

    let (program, latex, shell_escape) = match &engine {
        Engine::Script { path, .. } => {
            let mut args = vec![OsString::from(&name)];
            args.push(directives.parameter.clone().unwrap_or_default().into());
            let st = run_step(path, &args, &dir, &env, "engine script", sink, cancel, &mut report.steps)
                .map_err(|e| format!("cannot run {}: {e}", path.display()))?;
            report.passes = 1;
            finish(report, &dir, &stem, st == 0);
            return Ok(());
        }
        Engine::Context => {
            let bin = engine::find_binary(&opts.bin_dir, "context");
            let args: Vec<OsString> = vec!["--synctex=repeat".into(), "--nonstopmode".into(), name.clone().into()];
            let st = run_step(&bin, &args, &dir, &env, "context", sink, cancel, &mut report.steps)
                .map_err(|e| format!("cannot run context: {e}"))?;
            report.passes = 1;
            finish(report, &dir, &stem, st == 0);
            return Ok(());
        }
        Engine::Tex { program, latex, shell_escape } => (program.clone(), *latex, *shell_escape || opts.shell_escape),
    };
    let bin = engine::find_binary(&opts.bin_dir, &program);
    let text = read_lossy(&root).map_err(|e| e.to_string())?;

    // Optional precompiled preamble.
    let mut fmt: Option<PathBuf> = None;
    if opts.fast_preamble && latex {
        match prepare_format(&program, &text, &dir, &name, &cache, &env, &opts.bin_dir, &mut state, sink, cancel, &mut report.steps) {
            Ok(f) => {
                report.fast_preamble = Some("used".into());
                fmt = Some(f);
            }
            Err(why) => {
                sink(&format!("rtex: precompiled preamble skipped: {why}\n"));
                report.fast_preamble = Some(format!("skipped: {why}"));
            }
        }
    }

    let max = if opts.single_pass || !latex { 1 } else { opts.max_passes.max(1) };
    let mut pass = 0;
    let mut reason = String::from("initial pass");
    let mut retried_without_fmt = false;
    loop {
        pass += 1;
        report.passes = pass;
        let before = snapshot(&dir, &stem);
        let pass_start = SystemTime::now();
        let mut args: Vec<OsString> = vec!["-file-line-error".into(), "-interaction=nonstopmode".into()];
        if opts.synctex {
            args.push("-synctex=1".into());
        }
        if shell_escape {
            args.push("-shell-escape".into());
        }
        if let Some(f) = &fmt {
            args.push(format!("-fmt={}", f.display()).into());
        }
        args.extend(opts.extra_args.iter().map(OsString::from));
        args.push(name.clone().into());
        let status = run_step(&bin, &args, &dir, &env, &format!("pass {pass}: {reason}"), sink, cancel, &mut report.steps)
            .map_err(|e| format!("cannot run {}: {e}", bin.display()))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let summary = read_log(&dir, &stem);
        let has_errors = summary.as_ref().is_some_and(|s| s.errors().next().is_some()) || summary.as_ref().is_some_and(|s| s.fatal);

        // A preamble format can break documents in ways a normal run would
        // not; retry the pass once without it and remember the failure.
        if has_errors && fmt.is_some() && !retried_without_fmt {
            sink("rtex: errors with precompiled preamble, retrying without it\n");
            retried_without_fmt = true;
            state.fmt_broken = state.fmt_key;
            fmt = None;
            report.fast_preamble = Some("failed; fell back".into());
            pass -= 1;
            reason = "retry without precompiled preamble".into();
            continue;
        }
        report.log = summary.clone();
        if has_errors || status != 0 && summary.is_none() || pass >= max {
            if pass >= max && !has_errors && max > 1 && needs_rerun(&before, &snapshot(&dir, &stem), summary.as_ref()) {
                sink(&format!("rtex: stopped after {max} passes; output may still change\n"));
            }
            break;
        }

        // Decide whether another pass is needed.
        let mut why: Vec<String> = Vec::new();
        if opts.bibliography {
            if let Some(tool) = run_bibliography(&dir, &stem, &directives, &opts.bin_dir, &tool_env, pass_start, &mut state, sink, cancel, &mut report.steps) {
                why.push(format!("{tool} updated the bibliography"));
            }
        }
        if opts.index && run_index(&dir, &stem, &opts.bin_dir, &tool_env, pass_start, &mut state, sink, cancel, &mut report.steps) {
            why.push("makeindex updated the index".into());
        }
        let after = snapshot(&dir, &stem);
        if needs_rerun(&before, &after, summary.as_ref()) {
            why.push(changed_files(&before, &after).unwrap_or_else(|| "LaTeX asked for a rerun".into()));
        }
        if why.is_empty() || cancel.load(Ordering::Relaxed) {
            break;
        }
        reason = why.join("; ");
    }
    state.save(&cache);
    let ok = report.log.as_ref().is_some_and(|l| l.errors().next().is_none() && !l.fatal);
    finish(report, &dir, &stem, ok);
    Ok(())
}

fn finish(report: &mut BuildReport, dir: &Path, stem: &str, ok: bool) {
    if report.log.is_none() {
        report.log = read_log(dir, stem);
    }
    let pdf = dir.join(format!("{stem}.pdf"));
    if pdf.exists() {
        report.pdf = Some(pdf);
    }
    report.pages = report.log.as_ref().and_then(|l| l.pages);
    report.success = ok && report.pdf.is_some();
}

fn read_log(dir: &Path, stem: &str) -> Option<LogSummary> {
    read_lossy(&dir.join(format!("{stem}.log"))).ok().map(|t| log::parse(&t))
}

/// Environment for TeX runs: unwrapped logs, wide error context, texbin on PATH.
pub fn tex_env(bin_dir: &Path) -> Vec<(String, String)> {
    let mut env = tool_env(bin_dir);
    env.extend([
        ("max_print_line".into(), "100000".into()),
        ("error_line".into(), "254".into()),
        ("half_error_line".into(), "238".into()),
    ]);
    env
}

/// Environment for BibTeX/Biber/makeindex. They must not inherit the
/// enlarged `max_print_line` (BibTeX rejects it: "3 is a bad bad").
pub fn tool_env(bin_dir: &Path) -> Vec<(String, String)> {
    let path = std::env::var("PATH").unwrap_or_default();
    vec![("PATH".into(), format!("{}:{path}", bin_dir.display()))]
}

/// Run a helper tool and report whether `output` changed. The input key is
/// recorded even on failure so a broken .bib does not cause a rerun loop;
/// it is retried once its inputs change.
#[allow(clippy::too_many_arguments)]
fn run_helper(
    tool: &str,
    args: &[OsString],
    output: &Path,
    key: u64,
    slot: &mut u64,
    dir: &Path,
    bin_dir: &Path,
    env: &[(String, String)],
    reason: &str,
    sink: Sink,
    cancel: &AtomicBool,
    steps: &mut Vec<Step>,
) -> bool {
    let before = std::fs::read(output).ok().map(|b| fnv(&b));
    let bin = engine::find_binary(bin_dir, tool);
    if run_step(&bin, args, dir, env, reason, sink, cancel, steps).is_err() {
        return false;
    }
    *slot = key;
    let after = std::fs::read(output).ok().map(|b| fnv(&b));
    after.is_some() && after != before
}

// ---------------------------------------------------------------- reruns

const RERUN_EXTS: &[&str] = &["aux", "toc", "lof", "lot", "out", "nav", "snm", "loa", "thm", "bbl", "ind", "gls", "nls"];

fn snapshot(dir: &Path, stem: &str) -> HashMap<PathBuf, u64> {
    let mut files: Vec<PathBuf> = RERUN_EXTS.iter().map(|e| dir.join(format!("{stem}.{e}"))).collect();
    // `\include{chapters/two}` writes chapters/two.aux, listed in the main aux.
    if let Ok(aux) = std::fs::read(dir.join(format!("{stem}.aux"))) {
        for line in String::from_utf8_lossy(&aux).lines() {
            if let Some(rest) = line.strip_prefix("\\@input{") {
                if let Some(sub) = rest.strip_suffix('}') {
                    files.push(dir.join(sub));
                }
            }
        }
    }
    files.into_iter().filter_map(|f| std::fs::read(&f).ok().map(|b| (f, fnv(&b)))).collect()
}

fn needs_rerun(before: &HashMap<PathBuf, u64>, after: &HashMap<PathBuf, u64>, log: Option<&LogSummary>) -> bool {
    if log.is_some_and(|l| l.rerun_latex) {
        return true;
    }
    changed_files(before, after).is_some()
}

fn changed_files(before: &HashMap<PathBuf, u64>, after: &HashMap<PathBuf, u64>) -> Option<String> {
    let mut changed: Vec<String> = Vec::new();
    for (f, h) in after {
        let differs = match before.get(f) {
            Some(old) => old != h,
            None => is_meaningful_new(f),
        };
        if differs {
            changed.push(f.file_name().unwrap().to_string_lossy().into());
        }
    }
    changed.sort();
    (!changed.is_empty()).then(|| format!("{} changed", changed.join(", ")))
}

/// A newly created auxiliary file only forces a rerun if it holds something
/// the next pass will read (a fresh .aux of a label-free document does not).
fn is_meaningful_new(f: &Path) -> bool {
    let Ok(data) = std::fs::read(f) else { return false };
    if f.extension().is_some_and(|e| e == "aux") {
        let s = String::from_utf8_lossy(&data);
        ["\\newlabel", "\\bibcite", "\\@writefile", "\\contentsline"].iter().any(|k| s.contains(k))
    } else {
        data.iter().any(|b| !b.is_ascii_whitespace())
    }
}

// ---------------------------------------------------------- bibliography

fn modified_since(p: &Path, t: SystemTime) -> bool {
    std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|m| m >= t)
}

#[allow(clippy::too_many_arguments)]
fn run_bibliography(
    dir: &Path,
    stem: &str,
    directives: &crate::directives::Directives,
    bin_dir: &Path,
    env: &[(String, String)],
    pass_start: SystemTime,
    state: &mut State,
    sink: Sink,
    cancel: &AtomicBool,
    steps: &mut Vec<Step>,
) -> Option<&'static str> {
    let bcf = dir.join(format!("{stem}.bcf"));
    let bbl = dir.join(format!("{stem}.bbl"));
    let wants_biber = directives.bib_program.as_deref().is_some_and(|p| p.eq_ignore_ascii_case("biber"));
    if (bcf.exists() && modified_since(&bcf, pass_start)) || (wants_biber && bcf.exists()) {
        let bcf_text = read_lossy(&bcf).ok()?;
        let mut key = fnv(bcf_text.as_bytes());
        for src in bcf_datasources(&bcf_text) {
            key ^= fnv(&std::fs::read(dir.join(&src)).unwrap_or_default()).rotate_left(7);
        }
        if key == state.biber && bbl.exists() {
            return None;
        }
        let changed = run_helper("biber", &[stem.into()], &bbl, key, &mut state.biber, dir, bin_dir, env,
            "biber: citations or sources changed", sink, cancel, steps);
        return changed.then_some("biber");
    }

    // BibTeX: \citation/\bibdata/\bibstyle across main + sub aux files.
    let main_aux = read_lossy(&dir.join(format!("{stem}.aux"))).ok()?;
    let mut all = main_aux.clone();
    for line in main_aux.lines() {
        if let Some(sub) = line.strip_prefix("\\@input{").and_then(|r| r.strip_suffix('}')) {
            if let Ok(t) = read_lossy(&dir.join(sub)) {
                all.push_str(&t);
            }
        }
    }
    let relevant: Vec<&str> = all
        .lines()
        .filter(|l| l.starts_with("\\citation{") || l.starts_with("\\bibdata{") || l.starts_with("\\bibstyle{"))
        .collect();
    let bibdata = relevant.iter().find_map(|l| l.strip_prefix("\\bibdata{").and_then(|r| r.strip_suffix('}')))?;
    let mut key = fnv(relevant.join("\n").as_bytes());
    for b in bibdata.split(',') {
        let p = dir.join(if b.ends_with(".bib") { b.to_string() } else { format!("{b}.bib") });
        key ^= fnv(&std::fs::read(p).unwrap_or_default()).rotate_left(11);
    }
    if key == state.bib && bbl.exists() {
        return None;
    }
    let tool = directives.bib_program.clone().unwrap_or_else(|| "bibtex".into());
    let changed = run_helper(&tool, &[stem.into()], &bbl, key, &mut state.bib, dir, bin_dir, env,
        "bibtex: citations or .bib changed", sink, cancel, steps);
    changed.then_some("bibtex")
}

fn bcf_datasources(bcf: &str) -> Vec<String> {
    bcf.lines()
        .filter(|l| l.contains("<bcf:datasource"))
        .filter_map(|l| {
            let start = l.find('>')? + 1;
            let end = l.find("</bcf:datasource>")?;
            Some(l[start..end].trim().to_string())
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn run_index(
    dir: &Path,
    stem: &str,
    bin_dir: &Path,
    env: &[(String, String)],
    pass_start: SystemTime,
    state: &mut State,
    sink: Sink,
    cancel: &AtomicBool,
    steps: &mut Vec<Step>,
) -> bool {
    let idx = dir.join(format!("{stem}.idx"));
    if !modified_since(&idx, pass_start) {
        return false;
    }
    let key = fnv(&std::fs::read(&idx).unwrap_or_default());
    if key == state.index && dir.join(format!("{stem}.ind")).exists() {
        return false;
    }
    let mut args: Vec<OsString> = Vec::new();
    let ist = dir.join(format!("{stem}.ist"));
    if ist.exists() {
        args.extend(["-s".into(), ist.into_os_string()]);
    }
    args.push(format!("{stem}.idx").into());
    let ind = dir.join(format!("{stem}.ind"));
    run_helper("makeindex", &args, &ind, key, &mut state.index, dir, bin_dir, env,
        "makeindex: index entries changed", sink, cancel, steps)
}

// ------------------------------------------------------ precompiled preamble

/// Commands that open files or streams in the preamble; they do not survive
/// being dumped into a format, so such documents are compiled normally
/// unless they mark the dump point with `\endofdump`.
const FORMAT_UNSAFE: &[&str] = &[
    "\\makeindex", "\\makeglossaries", "\\makenomenclature", "\\newwrite", "\\openout",
    "\\tikzexternalize", "{minted}", "\\includeonly", "\\addbibresource", "{biblatex}",
];

#[allow(clippy::too_many_arguments)]
fn prepare_format(
    program: &str,
    text: &str,
    dir: &Path,
    name: &str,
    cache: &Path,
    env: &[(String, String)],
    bin_dir: &Path,
    state: &mut State,
    sink: Sink,
    cancel: &AtomicBool,
    steps: &mut Vec<Step>,
) -> Result<PathBuf, String> {
    if program != "pdflatex" {
        return Err(format!("only supported for pdflatex, not {program}"));
    }
    if !has_mylatexformat(bin_dir) {
        return Err("mylatexformat.ltx not installed".into());
    }
    let begin = find_begin_document(text).ok_or("no \\begin{document}")?;
    let preamble = &text[..begin];
    let code: String = preamble.lines().map(crate::outline::strip_comment).collect::<Vec<_>>().join("\n");
    let dump_part = code.split("\\endofdump").next().unwrap_or(&code);
    if let Some(bad) = FORMAT_UNSAFE.iter().find(|k| dump_part.contains(*k)) {
        return Err(format!("preamble uses {bad} (put it after \\endofdump to enable)"));
    }
    // Key: preamble text plus local class/package files it may load.
    let mut key = fnv(program.as_bytes()) ^ fnv(preamble.as_bytes()).rotate_left(3);
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut local: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "sty" || e == "cls" || e == "def"))
            .collect();
        local.sort();
        for p in local {
            key ^= fnv(&std::fs::read(&p).unwrap_or_default()).rotate_left(13);
        }
    }
    if state.fmt_broken == key {
        return Err("this preamble failed as a format before".into());
    }
    let fmt = cache.join("preamble");
    let fmt_file = cache.join("preamble.fmt");
    if fmt_file.exists() && state.fmt_key == key {
        return Ok(fmt);
    }
    std::fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    let pdftex = engine::find_binary(bin_dir, "pdftex");
    let args: Vec<OsString> = vec![
        "-ini".into(),
        "-interaction=nonstopmode".into(),
        "-jobname=preamble".into(),
        format!("-output-directory={}", cache.display()).into(),
        "&pdflatex".into(),
        "mylatexformat.ltx".into(),
        name.into(),
    ];
    let mut quiet = |_: &str| {};
    let st = run_step(&pdftex, &args, dir, env, "dump precompiled preamble", &mut quiet, cancel, steps)
        .map_err(|e| e.to_string())?;
    if st != 0 || !fmt_file.exists() {
        let _ = std::fs::remove_file(&fmt_file);
        state.fmt_broken = key;
        return Err("format dump failed".into());
    }
    sink("rtex: precompiled preamble rebuilt\n");
    state.fmt_key = key;
    Ok(fmt)
}

fn has_mylatexformat(bin_dir: &Path) -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        Command::new(engine::find_binary(bin_dir, "kpsewhich"))
            .arg("mylatexformat.ltx")
            .output()
            .is_ok_and(|o| o.status.success() && !o.stdout.is_empty())
    })
}

fn find_begin_document(text: &str) -> Option<usize> {
    let mut off = 0;
    for line in text.split_inclusive('\n') {
        let code = crate::outline::strip_comment(line);
        if let Some(i) = code.find("\\begin{document}") {
            return Some(off + i);
        }
        off += line.len();
    }
    None
}

// ------------------------------------------------------------ processes

#[allow(clippy::too_many_arguments)]
fn run_step(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    env: &[(String, String)],
    reason: &str,
    sink: Sink,
    cancel: &AtomicBool,
    steps: &mut Vec<Step>,
) -> io::Result<i32> {
    let t = Instant::now();
    let tool = program.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    sink(&format!("\n▶ {tool} — {reason}\n"));
    let status = run_process(program, args, cwd, env, sink, cancel)?;
    steps.push(Step {
        tool,
        args: args.iter().map(|a| a.to_string_lossy().into()).collect(),
        reason: reason.into(),
        status,
        millis: t.elapsed().as_millis() as u64,
    });
    Ok(status)
}

/// Run a process, streaming stdout+stderr to `sink`; kill it on cancel.
pub fn run_process(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    env: &[(String, String)],
    sink: Sink,
    cancel: &AtomicBool,
) -> io::Result<i32> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let readers: Vec<Box<dyn Read + Send>> = vec![
        Box::new(child.stdout.take().unwrap()),
        Box::new(child.stderr.take().unwrap()),
    ];
    let handles: Vec<_> = readers
        .into_iter()
        .map(|mut r| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 16384];
                while let Ok(n) = r.read(&mut buf) {
                    if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
    drop(tx);
    let mut pending: Vec<u8> = Vec::new();
    let mut killed = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(chunk) => {
                pending.extend_from_slice(&chunk);
                let valid = utf8_prefix_len(&pending);
                if valid > 0 {
                    sink(&String::from_utf8_lossy(&pending[..valid]));
                    pending.drain(..valid);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !killed && cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            killed = true;
        }
    }
    if !pending.is_empty() {
        sink(&String::from_utf8_lossy(&pending));
    }
    for h in handles {
        let _ = h.join();
    }
    let status = child.wait()?;
    Ok(status.code().unwrap_or(-1))
}

/// Length of the longest prefix that does not end inside a UTF-8 sequence.
fn utf8_prefix_len(b: &[u8]) -> usize {
    let n = b.len();
    for back in 1..=3.min(n) {
        let c = b[n - back];
        if c & 0xC0 == 0xC0 {
            let need = if c >= 0xF0 { 4 } else if c >= 0xE0 { 3 } else { 2 };
            return if back < need { n - back } else { n };
        }
        if c & 0x80 == 0 {
            return n;
        }
    }
    n
}

// --------------------------------------------------------------- state

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    bib: u64,
    biber: u64,
    index: u64,
    fmt_key: u64,
    fmt_broken: u64,
}

impl State {
    fn load(cache: &Path) -> State {
        std::fs::read(cache.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(&self, cache: &Path) {
        if std::fs::create_dir_all(cache).is_ok() {
            let _ = std::fs::write(cache.join("state.json"), serde_json::to_vec(self).unwrap());
        }
    }
}

/// Per-user cache root: `~/Library/Caches/rtex` on macOS.
pub fn cache_root() -> PathBuf {
    if let Some(x) = std::env::var_os("RTEX_CACHE_DIR") {
        return PathBuf::from(x);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into()));
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/rtex")
    } else if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(x).join("rtex")
    } else {
        home.join(".cache/rtex")
    }
}

pub fn doc_cache_dir(root: &Path) -> PathBuf {
    cache_root().join(format!("{:016x}", fnv(root.to_string_lossy().as_bytes())))
}

/// FNV-1a: stable across Rust versions (it is persisted in state.json).
pub fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split() {
        let s = "aé".as_bytes(); // 61 C3 A9
        assert_eq!(utf8_prefix_len(&s[..2]), 1);
        assert_eq!(utf8_prefix_len(s), 3);
    }

    #[test]
    fn begin_document_ignores_comments() {
        let t = "% \\begin{document}\n\\usepackage{x}\n\\begin{document}\nhi";
        assert_eq!(&t[find_begin_document(t).unwrap()..][..16], "\\begin{document}");
        assert!(find_begin_document(t).unwrap() > 20);
    }

    #[test]
    fn bcf_sources() {
        let bcf = "  <bcf:datasource type=\"file\" datatype=\"bibtex\" glob=\"false\">refs.bib</bcf:datasource>\n";
        assert_eq!(bcf_datasources(bcf), vec!["refs.bib"]);
    }
}
