//! `rtex` — command-line front end to rtex-core.

use clap::{Parser, Subcommand};
use rtex_core::log::Level;
use rtex_core::{build, synctex, BuildOptions, BuildReport};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "rtex", version, about = "Fast LaTeX build, SyncTeX and log tools (TeXShop-compatible)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone)]
struct BuildArgs {
    /// Any file of the project; `% !TEX root` is followed.
    file: PathBuf,
    /// Engine (overrides `% !TEX program`): pdflatex, xelatex, lualatex, pdftex, context, or a .engine name.
    #[arg(short, long)]
    engine: Option<String>,
    /// Maximum LaTeX passes.
    #[arg(long, default_value_t = 5)]
    passes: u32,
    /// One engine run only (TeXShop's plain Typeset behaviour).
    #[arg(long)]
    single: bool,
    /// Reuse a precompiled preamble (pdflatex + mylatexformat).
    #[arg(short = 'f', long)]
    fast: bool,
    #[arg(long)]
    shell_escape: bool,
    #[arg(long)]
    no_synctex: bool,
    #[arg(long)]
    no_bib: bool,
    #[arg(long)]
    no_index: bool,
    /// Directory holding the TeX binaries.
    #[arg(long, default_value = rtex_core::engine::DEFAULT_BIN_DIR)]
    bin_dir: PathBuf,
    /// Print the engine output while it runs.
    #[arg(short, long)]
    verbose: bool,
    /// Print the report as JSON.
    #[arg(long)]
    json: bool,
    /// Open the PDF when done.
    #[arg(long)]
    open: bool,
    /// Build files afterwards: keep, hide (move to the cache, restored next run) or delete.
    #[arg(long, default_value = "keep", value_parser = ["keep", "hide", "delete"])]
    after: String,
}

impl BuildArgs {
    fn options(&self) -> BuildOptions {
        BuildOptions {
            engine: self.engine.clone(),
            max_passes: self.passes,
            single_pass: self.single,
            synctex: !self.no_synctex,
            shell_escape: self.shell_escape,
            fast_preamble: self.fast,
            bibliography: !self.no_bib,
            index: !self.no_index,
            bin_dir: self.bin_dir.clone(),
            extra_args: Vec::new(),
            after_build: rtex_core::clean::AfterBuild::parse(&self.after).unwrap_or_default(),
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Typeset a document (smart reruns, BibTeX/Biber, makeindex).
    Build(BuildArgs),
    /// Rebuild whenever a source file in the project changes.
    Watch(BuildArgs),
    /// SyncTeX: source → PDF.
    Forward {
        /// The PDF (or .synctex.gz).
        pdf: PathBuf,
        /// Source file.
        source: PathBuf,
        line: u32,
        #[arg(long)]
        page_hint: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// SyncTeX: PDF point → source line. x/y in PDF points from the page's top-left.
    Inverse {
        pdf: PathBuf,
        page: u32,
        x: f64,
        y: f64,
        #[arg(long)]
        json: bool,
    },
    /// Summarise errors, warnings and bad boxes from a .log file.
    Log {
        log: PathBuf,
        #[arg(long)]
        json: bool,
        /// Include bad boxes.
        #[arg(long)]
        badboxes: bool,
    },
    /// Sections, labels and `%:` marks of a source file.
    Outline {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Show the root file and `% !TEX` directives that apply to a file.
    Info { file: PathBuf },
    /// Remove auxiliary files.
    Clean {
        file: PathBuf,
        /// All auxiliary files in the folder tree, not just the root's.
        #[arg(long)]
        aggressive: bool,
    },
    /// List built-in engines and installed .engine scripts.
    Engines,
    /// Job postings: capture, tailor the resume, list.
    Job {
        /// Job-search folder (default: $RTEX_JOBS_ROOT or ~/Projects/career-job-search).
        #[arg(long, global = true)]
        root: Option<PathBuf>,
        #[command(subcommand)]
        cmd: JobCmd,
    },
    /// Show command completions for a word (TeXShop CommandCompletion.txt).
    Complete {
        word: String,
        /// Use this completion file instead of the default search path.
        #[arg(long)]
        file: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum JobCmd {
    /// List jobs by stage.
    List,
    /// Capture a posting into jobs/inbox: from a URL, or pasted text (--text FILE, or - for stdin).
    Add {
        url: Option<String>,
        #[arg(long)]
        text: Option<PathBuf>,
        #[arg(long, default_value = "")]
        title: String,
        #[arg(long, default_value = "")]
        company: String,
        #[arg(long, default_value = "")]
        location: String,
    },
    /// Tailor the resume for a job with Claude, compile it and check it.
    Tailor {
        slug: String,
        /// Leave the job in its stage (by default a ready resume moves it to apply-now).
        #[arg(long)]
        no_move: bool,
        /// Also write the role analysis (slower; off by default).
        #[arg(long)]
        analysis: bool,
    },
    /// Show the context files sent to Claude for a job.
    Context { slug: String },
    /// Check a compiled resume against the 116-character line rule.
    Lines {
        pdf: PathBuf,
        #[arg(long)]
        base: Option<PathBuf>,
    },
}

fn job_cmd(root: Option<PathBuf>, cmd: JobCmd) -> Result<ExitCode, String> {
    use rtex_jobs::{capture, context, linecheck, tailor, Workspace};
    if let JobCmd::Lines { pdf, base } = &cmd {
        let v = linecheck::check(pdf, base.as_deref())?;
        for x in &v {
            println!("{} {}", if x.in_base { "base" } else { "FAIL" }, x.describe());
        }
        let fails = v.iter().filter(|x| !x.in_base).count();
        println!("line check: {}", if fails == 0 { "ok".to_string() } else { format!("{fails} to fix") });
        return Ok(if fails == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) });
    }
    let ws = Workspace::open(&root.unwrap_or_else(Workspace::default_root)).map_err(|e| e.to_string())?;
    match cmd {
        JobCmd::List => {
            for stage in rtex_jobs::workspace::STAGES {
                let jobs: Vec<_> = ws.jobs().into_iter().filter(|j| j.stage == *stage).collect();
                if jobs.is_empty() {
                    continue;
                }
                println!("{stage} ({})", jobs.len());
                for j in jobs {
                    let pdf = if j.resume_pdf.is_some() { "  [pdf]" } else { "" };
                    println!("  {}  {} — {}{pdf}", j.slug, j.company, j.title);
                }
            }
        }
        JobCmd::Add { url, text, title, company, location } => {
            let posting = match (&text, &url) {
                (Some(path), _) => {
                    let body = if path.as_os_str() == "-" {
                        let mut s = String::new();
                        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
                        s
                    } else {
                        std::fs::read_to_string(path).map_err(|e| e.to_string())?
                    };
                    capture::from_text(&body, &title, &company, &location, url.as_deref().unwrap_or(""))
                }
                (None, Some(u)) => {
                    let mut p = capture::capture_url(u)?;
                    if !title.is_empty() { p.title = title; }
                    if !company.is_empty() { p.company = company; }
                    if !location.is_empty() { p.location = location; }
                    p
                }
                (None, None) => return Err("give a URL or --text FILE".into()),
            };
            let job = ws.create(&posting).map_err(|e| e.to_string())?;
            println!("added {} ({} — {}, {} words)", job.slug, job.company, job.title, posting.body.split_whitespace().count());
        }
        JobCmd::Context { slug } => {
            let job = ws.find(&slug).ok_or(format!("no job {slug}"))?;
            let sel = context::selected(&ws, &job);
            for f in context::available(&ws) {
                let mark = if sel.contains(&f.path) { "x" } else { " " };
                println!("[{mark}] {:>7}  {}  ({})", f.bytes, f.path, f.note);
            }
        }
        JobCmd::Tailor { slug, no_move, analysis } => {
            let job = ws.find(&slug).ok_or(format!("no job {slug}"))?;
            let mut progress = |ev: &serde_json::Value| match ev["kind"].as_str() {
                Some("edit") => {
                    let e = &ev["edit"];
                    eprintln!(
                        "  + edit {} [{}] {}: {}",
                        ev["index"].as_u64().unwrap_or(0) + 1,
                        e["op"].as_str().unwrap_or("replace"),
                        e["entry"].as_str().unwrap_or(""),
                        e["rationale"].as_str().unwrap_or("")
                    );
                }
                _ => eprintln!("· {}", ev["text"].as_str().unwrap_or("")),
            };
            let topts = tailor::TailorOptions { analysis };
            let out = tailor::tailor(&ws, &job, &topts, &mut progress)?;
            println!("decision: {}{}", out.decision, if out.stop_reason.is_empty() { String::new() } else { format!(" ({})", out.stop_reason) });
            println!("edits: {} ({} not applied)", out.edits.len(), out.unapplied.len());
            println!("job: {} — {}", out.company, out.position);
            if let Some(pdf) = out.attachment.as_ref().or(out.pdf.as_ref()) {
                println!("resume: {} ({} page(s))", pdf.display(), out.pages.unwrap_or(0));
            }
            let fails = out.violations.iter().filter(|v| !v.in_base).count();
            println!("line check: {}", if fails == 0 { "ok".to_string() } else { format!("{fails} to fix") });
            for c in &out.check_claims {
                println!("check claim: {c}");
            }
            let calls: Vec<String> = out
                .calls
                .iter()
                .map(|c| format!("{} {:.0}s, {} tok out, {}K of {}K in cached", c.purpose, c.reply.seconds, c.reply.output_tokens, c.reply.cached_input_tokens / 1000, c.reply.input_tokens / 1000))
                .collect();
            println!("{:.0} s total ({}), ${:.2} list price, model {}", out.seconds, calls.join(", "), out.cost_usd, out.model);

            if out.ready() && !no_move && job.stage == "inbox" {
                let moved = ws.move_job(&job, "apply-now").map_err(|e| e.to_string())?;
                println!("moved to {}", moved.dir.display());
            }
        }
        JobCmd::Lines { .. } => unreachable!(),
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rtex: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.cmd {
        Cmd::Build(a) => {
            let report = do_build(&a);
            Ok(if report.success { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::Watch(a) => watch(a),
        Cmd::Forward { pdf, source, line, page_hint, json } => {
            let t = Instant::now();
            let s = synctex::Scanner::open(&pdf).map_err(|e| e.to_string())?;
            let source = rtex_core::root::absolutize(&source).map_err(|e| e.to_string())?;
            let r = s.forward(&source, line, page_hint).ok_or("no match")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r).unwrap());
            } else {
                println!("page {} (line {})", r.page, r.matched_line);
                for b in &r.rects {
                    println!("  x={:.2} y={:.2} w={:.2} h={:.2}", b.x, b.y, b.width, b.height);
                }
                eprintln!("({:.2} ms)", t.elapsed().as_secs_f64() * 1e3);
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Inverse { pdf, page, x, y, json } => {
            let t = Instant::now();
            let s = synctex::Scanner::open(&pdf).map_err(|e| e.to_string())?;
            let r = s.inverse(page, x, y).ok_or("no match")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r).unwrap());
            } else {
                println!("{}:{}", r.file.display(), r.line);
                eprintln!("({:.2} ms)", t.elapsed().as_secs_f64() * 1e3);
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Log { log, json, badboxes } => {
            let text = rtex_core::root::read_lossy(&log).map_err(|e| e.to_string())?;
            let s = rtex_core::log::parse(&text);
            if json {
                println!("{}", serde_json::to_string_pretty(&s).unwrap());
            } else {
                print_messages(&s, badboxes);
            }
            Ok(if s.errors().next().is_some() { ExitCode::from(1) } else { ExitCode::SUCCESS })
        }
        Cmd::Outline { file, json } => {
            let text = rtex_core::root::read_lossy(&file).map_err(|e| e.to_string())?;
            let o = rtex_core::outline::outline(&text);
            if json {
                println!("{}", serde_json::to_string_pretty(&o).unwrap());
            } else {
                for e in o {
                    let indent = "  ".repeat(e.level.min(5) as usize);
                    println!("{:>5}  {indent}{:?}: {}", e.line, e.kind, e.title);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Info { file } => {
            let (root, d) = rtex_core::root::resolve_root(&file).map_err(|e| e.to_string())?;
            println!("root:      {}", root.display());
            println!("program:   {}", d.program.as_deref().unwrap_or("(default pdflatex)"));
            if let Some(e) = d.encoding {
                println!("encoding:  {e}");
            }
            if let Some(b) = d.bib_program {
                println!("bibtool:   {b}");
            }
            if let Some(p) = d.parameter {
                println!("parameter: {p}");
            }
            println!("cache:     {}", rtex_core::build::doc_cache_dir(&root).display());
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Clean { file, aggressive } => {
            let root = rtex_core::root::resolve_root(&file).map(|(r, _)| r).map_err(|e| e.to_string())?;
            let removed = rtex_core::clean::clean(&root, aggressive, &[]).map_err(|e| e.to_string())?;
            for p in &removed {
                println!("removed {}", p.display());
            }
            println!("{} file(s) removed", removed.len());
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Complete { word, file } => {
            let loaded = match file {
                Some(f) => std::sync::Arc::new(rtex_core::completion::load(&f).map_err(|e| e.to_string())?),
                None => rtex_core::completion::active(),
            };
            println!("source: {} ({} entries)", loaded.source, loaded.table.entries.len());
            for (i, e) in loaded.table.candidates(&word).iter().enumerate() {
                let x = rtex_core::completion::expand(&e.body, "");
                let kind = if e.key.is_some() { "abbrev" } else { "complete" };
                println!("{:>3}. [{kind}] {}", i + 1, x.text.replace('\n', "⏎"));
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Job { root, cmd } => job_cmd(root, cmd),
        Cmd::Engines => {
            for e in rtex_core::engine::list() {
                match e.path {
                    Some(p) => println!("{:<16} {:<14} {}", e.name, e.kind, p.display()),
                    None => println!("{:<16} {}", e.name, e.kind),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn do_build(a: &BuildArgs) -> BuildReport {
    let cancel = AtomicBool::new(false);
    let verbose = a.verbose;
    let mut sink = |s: &str| {
        if verbose {
            eprint!("{s}");
        }
    };
    let report = build(&a.file, &a.options(), &mut sink, &cancel);
    if a.json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        print_report(&report);
    }
    if let (true, Some(pdf)) = (a.open, &report.pdf) {
        let _ = std::process::Command::new("open").arg(pdf).status();
    }
    report
}

fn print_report(r: &BuildReport) {
    for s in &r.steps {
        let mark = if s.status == 0 { "✓" } else { "✗" };
        println!("{mark} {:<10} {:>6} ms  {}", s.tool, s.millis, s.reason);
    }
    if let Some(log) = &r.log {
        print_messages(log, false);
    }
    if let Some(e) = &r.error {
        println!("error: {e}");
    }
    if let Some(f) = &r.fast_preamble {
        println!("precompiled preamble: {f}");
    }
    let status = if r.cancelled { "cancelled" } else if r.success { "ok" } else { "FAILED" };
    let pages = r.pages.map(|p| format!(", {p} page{}", if p == 1 { "" } else { "s" })).unwrap_or_default();
    println!(
        "{status}: {} pass{} in {} ms{pages}{}",
        r.passes,
        if r.passes == 1 { "" } else { "es" },
        r.total_millis,
        r.pdf.as_ref().map(|p| format!(" → {}", p.display())).unwrap_or_default()
    );
}

fn print_messages(s: &rtex_core::log::LogSummary, badboxes: bool) {
    for m in &s.messages {
        if m.level == Level::BadBox && !badboxes {
            continue;
        }
        let tag = match m.level {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::BadBox => "badbox",
        };
        let loc = match (&m.file, m.line) {
            (Some(f), Some(l)) => format!("{f}:{l}: "),
            (Some(f), None) => format!("{f}: "),
            (None, Some(l)) => format!("line {l}: "),
            _ => String::new(),
        };
        println!("{loc}{tag}: {}", m.message);
        if let Some(c) = &m.context {
            println!("    {c}");
        }
    }
    let bad = s.count(Level::BadBox);
    if bad > 0 && !badboxes {
        println!("({bad} bad box{} hidden; see `rtex log --badboxes`)", if bad == 1 { "" } else { "es" });
    }
}

fn watch(a: BuildArgs) -> Result<ExitCode, String> {
    use notify::{RecursiveMode, Watcher};
    let (root, _) = rtex_core::root::resolve_root(&a.file).map_err(|e| e.to_string())?;
    let dir = root.parent().unwrap().to_path_buf();
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if matches!(ev.kind, notify::EventKind::Modify(_) | notify::EventKind::Create(_)) {
                for p in ev.paths {
                    let _ = tx.send(p);
                }
            }
        }
    })
    .map_err(|e| e.to_string())?;
    watcher.watch(&dir, RecursiveMode::Recursive).map_err(|e| e.to_string())?;
    println!("watching {} (Ctrl-C to stop)", dir.display());
    do_build(&a);
    loop {
        let first = rx.recv().map_err(|e| e.to_string())?;
        if !is_source(&first) {
            continue;
        }
        // Debounce: editors write several events per save.
        let mut changed = vec![first];
        while let Ok(p) = rx.recv_timeout(Duration::from_millis(80)) {
            changed.push(p);
        }
        let names: Vec<_> = changed
            .iter()
            .filter(|p| is_source(p))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        println!("\n── changed: {}", names.join(", "));
        do_build(&a);
        // Swallow events caused by our own build output.
        while rx.recv_timeout(Duration::from_millis(30)).is_ok() {}
    }
}

fn is_source(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()),
        Some("tex" | "bib" | "sty" | "cls" | "ltx" | "dtx" | "bst" | "png" | "jpg" | "jpeg" | "eps" | "svg" | "tikz" | "csv" | "dat")
    ) || p.extension().is_some_and(|e| e == "pdf") && !p.with_extension("synctex.gz").exists()
}
