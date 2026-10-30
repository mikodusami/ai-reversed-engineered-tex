# RTeX

A TeXShop-style LaTeX workbench rebuilt from a reverse-engineered TeXShop
5.57: a Rust core for the work that has to be fast, and a native macOS
front end in Objective-C++ (AppKit + PDFKit).

Like TeXShop, RTeX does not reimplement TeX itself. It drives the TeX Live
engines (pdflatex, xelatex, lualatex, ConTeXt) and owns everything around
them: build orchestration, log analysis, SyncTeX, outline and highlighting.

```
crates/rtex-core   Rust library (also a C static library for the app)
crates/rtex-jobs   job postings → tailored, compiled, checked resumes (uses Claude Code)
crates/rtex-cli    `rtex` command-line tool
app/               RTeX.app (Objective-C++), links librtex_core.a
docs/RE-NOTES.md   what was recovered from TeXShop and how it maps here
scripts/           SyncTeX check against the reference CLI; build benchmark
fixtures/          test documents
```

## Build

Requires Rust (`~/.cargo/bin`), Xcode command-line tools, and MacTeX
(`/Library/TeX/texbin`).

```sh
cargo build --release          # → target/release/rtex
cargo test
make -C app                    # → app/build/RTeX.app
make -C app run
make -C app test               # headless editor tests
```

## CLI

```sh
rtex build paper.tex            # smart build: reruns, BibTeX/Biber, makeindex as needed
rtex build -f paper.tex         # + precompiled preamble (pdflatex)
rtex build --single paper.tex   # one engine run, like TeXShop's plain Typeset
rtex build --after hide paper.tex   # leave only sources + PDF in the folder (also: keep, delete)
rtex watch paper.tex            # rebuild on save
rtex forward paper.pdf chapter.tex 42      # SyncTeX source → PDF
rtex inverse paper.pdf 3 200 400           # SyncTeX PDF point → source line
rtex log paper.log [--badboxes]
rtex outline paper.tex
rtex info chapters/one.tex      # root file and % !TEX directives
rtex clean paper.tex [--aggressive]
rtex engines                    # built-ins + ~/Library/TeXShop/Engines/*.engine
rtex complete '{eq'             # command completions, best first
```

`% !TEX root`, `% !TEX program` / `TS-program`, `% !TEX parameter`,
`% !TEX encoding` and `% !BIB TS-program` work as in TeXShop, and TeXShop
`.engine` scripts run unchanged.

## Jobs (tailoring engine)

Works on a job-search folder (default `~/Projects/career-job-search`, or `--root` /
`$RTEX_JOBS_ROOT`) with `resumes/base.tex` (+ `base.pdf`), optionally `resumes/base-data.tex`
(+ `base-data.pdf`, the base for data roles), a `background/` folder, and `jobs/<stage>/<job>/`. Each job folder holds only:

```
page.txt                                                    the job page's text (Source URL on top)
attachments/<company>.<position>.<owner>.<MM-DD-YYYY>.pdf   the tailored resume to upload
.tailor.json                                                (hidden) what changed, checks, timing
```

The tailored `.tex` is kept next to a copy of the PDF in `resumes/tailored/` (same name).

```sh
rtex job add https://…/job/123          # saves the page text (Workable, Greenhouse, Lever and
                                        # Ashby links are read through their job APIs)
pbpaste | rtex job add --text - --title "Acme Software Engineer"   # pages behind a sign-in
rtex job tailor <job>                   # tailor, compile, check
rtex job context <job>                  # files sent to Claude
rtex job list
rtex job lines resume.pdf --base base.pdf   # the 116 / 47-character line rule
```

How `tailor` works: the Claude Code CLI on this Mac, no tools, no MCP servers. The
system prompt is your tailoring instructions, the output rules, **all of `background/`**
(PDFs as their text, extracted once and cached), `base.tex`, and how each base bullet
prints today. It's identical for every job, so Claude's prompt cache serves it from the
second job on (measured: 109K of 111K input tokens cached, $1.04 → $0.18 per job). The
message is just the job page. Claude returns only edits (replace / delete / insert
entries) plus the company and position for the file name; they stream in as written.
With two bases, both are sent and Claude names the one it edits (`base-data.tex` for
data roles). RTeX applies the edits to a copy of that base, compiles, checks one page and the line rule,
and sends only the failing edits back at low effort. New Skills terms that no resume
or background file mentions are flagged as claims to check. Typical run: about a
minute.

A base PDF older than its `.tex` is recompiled before tailoring. Instructions are read from `<root>/tailoring.md`, `<root>/auto-apply/tailoring.md`, or
`~/Library/Application Support/RTeX/tailoring.md`. Optional settings in
`<root>/rtex-jobs.json`: `owner` (name in file names; default: last name in base.tex),
`model`, `effort`, `fix_effort` (default `low`), `fix_rounds` (default 2), `analysis`
(write the role analysis; default false), `hard_gates` (let Claude refuse jobs that
fail a hard requirement; default false: every job gets a resume).

### Jobs window (⇧⌘J)

A list of jobs grouped by stage (Inbox, Ready to Apply, Hold for Referral, Applied,
Do Not Apply), read straight from the job folders. For the selected job:

* **Resume**: the tailored PDF next to `base.pdf`.
* **Changes**: the checks and every edit (original vs. replacement), streaming in while
  Claude writes; claims to verify in orange.
* **Page**: the saved page text. **Context**: which files Claude reads (all by default).

Toolbar: **New Job** (URL, or pasted text when the page needs a sign-in; tailors right
away by default), **Tailor**, **Tailor Inbox** (one job after another in the
background), **Move To**, **Apply Page** (opens the application in your browser),
**Show PDF** (the file to upload, in Finder), **Edit Resume** (the tailored `.tex` in
RTeX; the upload copy is refreshed after you recompile), **Delete** (moves the job's
folder to the Trash). Jobs → Choose Job Folder… picks the folder. Nothing is ever
submitted for you.

## App

* Editor with LaTeX highlighting (incremental, done in Rust), line numbers,
  comment/uncomment (⌘/), shift (⌘] / ⌘[), find bar.
* Command completion, TeXShop-compatible: type a prefix or abbreviation and
  press Esc (again to cycle). Uses `~/Library/TeXShop/CommandCompletion/CommandCompletion.txt`
  (or `~/Library/RTeX/CommandCompletion.txt`, or TeXShop's bundled file).
  Marks: ⌃⌘F / ⌃⌘G next/previous (add ⌥ to delete), ⌘8 insert mark.
* Auto-pairing (Edit → Auto-Pair Brackets): `{ [ (` and TeXShop's
  `^ _ "` pairs, typing over the closer, Backspace removes an empty pair,
  `{`/`$` wrap a selection, `≤`→`\leq` style shortcuts from TeXShop's
  `autocompletion.plist`. Esc right after `\begin{name}` adds `\end{name}`.
* Saving works like TeXShop's: autosave and Versions (File → Save Options to
  turn off), the `% !TEX encoding` line decides the encoding at every save
  (⌥ ignores it), files changed by other apps are reloaded, edited
  `\input`/`\include` files of the project are saved before typesetting,
  optional `name~.tex` backups and save-panel encoding menu. Unlike TeXShop,
  RTeX never drops characters that don't fit the encoding and never closes
  a window because its file was moved or deleted.
* Build panel (toolbar **Build**, or ⇧⌘B): what happens to build files after
  each typeset — *Keep*, *Hide* (folder shows only `.tex`/`.pdf`; build
  files wait in `~/Library/Caches/rtex`, so rebuilds stay incremental) or
  *Delete* (every typeset starts from scratch). Sources (`.bib`, images,
  `.sty`) are never touched and ⌘-click SyncTeX works in all modes. The
  panel also holds the typesetting toggles and "Trash Build Files Now".
* ⌘T typesets in the background, with live console output and a Stop button.
  Errors and warnings appear as clickable `file:line` links; ⌘' steps through them.
* PDF preview beside the source. It reloads in place after a typeset and
  picks up PDFs rebuilt elsewhere (e.g. by `rtex watch`).
* SyncTeX: ⌘-click in the source to highlight that line in the PDF, ⌘-click
  in the PDF to jump to the source (other files of the project are opened).
* Engine popup (built-ins plus your `.engine` scripts), and a Tags popup
  (sections, `%:` marks, labels).
* Typeset menu toggles: precompiled preamble, single pass, typeset on save,
  shell escape. Trash Aux Files (⌥ for the whole folder).

## Speed

Measured on this machine (Apple silicon, TeX Live 2026):

| | TeXShop's way | RTeX |
|---|---|---|
| SyncTeX query | spawn `synctex`: ~2.8 ms | in process: ~3.5 µs (parse once ~1.2 ms) |

Build, 56-page book with tikz + hyperref (`scripts/bench_build.py`, median of 3 runs):

| | fresh build | text edit | new label |
|---|---|---|---|
| latexmk (TeXShop `pdflatexmk`) | 2.65 s | 0.93 s | 1.80 s |
| `rtex build` | 2.42 s | 0.81 s | 1.63 s |
| `rtex build --fast` | 2.35 s | 0.67 s | 1.27 s |

Where the build time goes: pdflatex itself dominates, so RTeX's gains come
from doing less work:

* passes are rerun only when `.aux`/`.toc`/`.out`/... content actually changed;
* BibTeX, Biber and makeindex run only when their inputs changed (the keys
  survive between builds);
* there is no Perl start-up;
* the preamble can optionally be precompiled. This is skipped automatically
  when the preamble opens files (`\makeindex`, glossaries, minted, ...) and
  falls back to a normal pass if the format misbehaves.

## Not done (TeXShop features left out)

Macros editor, LaTeX/Matrix panels, BibDesk completion,
spell-check languages per file, AppleScript, HTML preview, multiple-window
preview modes, ConTeXt/old-style sync parsers, glossaries/nomenclature
tools in the smart build.
