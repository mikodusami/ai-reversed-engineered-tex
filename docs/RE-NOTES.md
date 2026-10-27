# TeXShop 5.57 — reverse-engineering notes

What was recovered from `/Applications/TeX/TeXShop.app` with REA, and how each
piece maps onto RTeX. TeXShop is GPL software; these notes describe behaviour
and structure to reimplement it, not copied code.

## Target

| | |
|---|---|
| Bundle | `/Applications/TeX/TeXShop.app`, version 5.57 |
| Binary | universal Mach-O (x86_64 + arm64); arm64 slice sha256 `3895524f0d653ff5a120c9efca64fe9dd1064151d5b11e4314345259a658cc5d` |
| Linked | Cocoa, Carbon, Quartz (PDFKit), WebKit, AVFoundation/AVKit (weak), Sparkle 1.24, OgreKit (regex find) |

REA tools used: `open_binary`, `inspect_macho` (load commands and dependencies),
`inspect_plist` (`FactoryDefaults.plist`), `decode_interface_builder` (nib
archives), `observe_native_ui` / `capture_native_ui_scenario` (to drive and
check the RTeX build). No decompiler provider (Ghidra, Hopper, IDA) is
installed on this machine, so there is no pseudocode. The analysis rests on
Objective-C metadata (class and selector tables from `__objc_classname` and
`__objc_methname`), the string table, the shipped resources, and the
behaviour of the external tools TeXShop drives.

## Architecture (from the Objective-C class table)

TeXShop is one large `TSDocument` class split into categories, plus helper windows:

| TeXShop class / category | Role | RTeX |
|---|---|---|
| `TSDocument` (+ `JobProcessing`) | typeset jobs through `NSTask`: `doTypeset:`, `doBibtex:`, `handleMakeindexCommand:`, `checkRootFile:forTask:` | `rtex-core::build` |
| `RootFile`, `% !TEX root` strings | root-file redirection | `rtex-core::root` |
| `SyntaxHighlighting` | colouring (keys `commandred/green/blue`, `commentred…`, `markerred…`, `indexred…`) | `rtex-core::highlight` + `RTDocument` temporary attributes |
| `SyncTeX`, `SyncOld`, `SyncConTeXt` | sync: `doSyncTeXForPage:x:y:yOriginal:`, `doPreviewSyncTeXWithFilename:andLine:…` plus `_synctexTask` / `_synctexPipe` / `_forwardSyncTask` (spawns the `synctex` tool) | `rtex-core::synctex` (in-process) |
| `TSLogWindow`, `texloganalyserwrap` | log display and analysis | `rtex-core::log` |
| `TSConsoleWindow` | engine output | console pane in `RTDocument` |
| `MyPDFKitView`, `TSPreviewWindow` | PDF preview (`_pdfRefreshTimer`, `refreshPDFWindow:`) | `RTPDFView` |
| `NoodleLineNumberView` | line numbers | `RTLineNumberRuler` |
| Tags / Labels menus (`doTag:`, `doLabel:`, `_tagTimer`) | outline popups, `%:` user marks | `rtex-core::outline`, Tags toolbar popup |
| Command completion (`CommandCompletion.txt`, `CommandCompletionChar = ESCAPE`) | Esc completion, Marks menu | `rtex-core::completion`, `RTTextView complete:` |
| `TSMacro*`, `TSMatrixPanelController`, `TSLaTeXPanelController`, `BibDeskCompletion` | macros, panels | not reimplemented |

## Magic comments (string table)

`% !TEX root =`, `% !TEX TS-program =`, `% !TEX program =`, `% !TEX encoding =`,
`% !TEX parameter =`, `% !TEX TS-parameter =`, `% !TEX spellcheck =`,
`% !BIB TS-program =`, the same keys spelled `%!TEX …`, and bare flags
(`useTabs`, `pdfSinglePage`, `!TEX-noPreview` …). All of them are parsed in
`directives.rs`; root, program, encoding, parameter and BIB program are acted on.

## Typesetting defaults (`FactoryDefaults.plist`)

```
TetexBinPath   = /Library/TeX/texbin
LatexCommand   = pdflatex --file-line-error --synctex=1
TexCommand     = pdftex --file-line-error --synctex=1
BibTeXengine   = bibtex
DefaultEngine  = (user default; TeXShop ships XeLaTeX here)
AlternateEngine= pdflatexmk
AggressiveTrashAUX, OtherTrashExtensions → Trash AUX Files
```

RTeX keeps the bin path and the `-file-line-error -synctex=1` flags. It adds
`-interaction=nonstopmode`, because the console is output-only, and sets
`max_print_line=100000` so log lines are never wrapped. BibTeX, Biber and
makeindex get the plain environment: BibTeX fails its start-up consistency
check ("3 is a bad bad") when `max_print_line` is raised.

## Engines

`~/Library/TeXShop/Engines/*.engine` are shell scripts run as
`script "<file name>" "<parameter>"` from the root's folder (the bundled
`XeLaTeX.engine` is `xelatex -file-line-error -synctex=1 "$1"`). RTeX:

* runs those scripts the same way, so existing engines keep working;
* maps `pdflatexmk`, `xelatexmk`, `lualatexmk` (and the `se…` shell-escape
  variants), which call latexmk in TeXShop, onto its own pipeline instead.

## SyncTeX

TeXShop answers sync requests by spawning `synctex view` / `synctex edit`
through NSTask and NSPipe. RTeX parses `.synctex.gz` once, caches it by
modification time, and answers queries in memory.

The inverse (edit) query ports the algorithm of the reference parser,
`synctex_parser.c` 1.21 (MIT, © Jérôme Laurens). The details that matter:

* every h-box gets synthetic boundary records at both ends; the leading one
  takes the line of the first real child;
* h-boxes have *visible* extents, grown to cover kerns, glue, math and child boxes;
* leading `x` records of a box take the tag/line of the next non-`x` node;
* a trailing kern + glue pair (end of paragraph) is credited to the material before it;
* the containing h-box is searched in reverse closing order and the smallest
  match wins; the hit is resolved through `deepest_container` and
  `closest_children_in_hbox` with the reference's nine-region distance.

Check: `scripts/synctex_oracle.py` compares RTeX with the installed `synctex`
CLI on a grid of points and on every source line.

| fixture | inverse (identical) | forward page | forward box |
|---|---|---|---|
| basic (3 pages, index, hyperref) | 1200 / 1200 | 30 / 30 | 29 / 30 |
| multi (`\input` + `\include`) | 800 / 800 | 14 / 14 | 14 / 14 |
| biblio (BibTeX) | 400 / 400 | 6 / 6 | 6 / 6 |

## Command completion

Source: `Resources/TeXShop/CommandCompletion/` (the data file plus the
"Command Completion For TeXShop" guide). The rules are covered by tests in `completion.rs`:

* `key:=expansion` lines are abbreviations (exact match); other lines are
  completions (prefix match). The word starts after the last of
  `space tab newline . , ; : { } ( ) \`; a leading `\` or `{` is part of it.
* The file is searched from the bottom. The guide's examples only hold that way:
  `\b` → `\begin{` (line 451, after `\boxed{` on line 363), and `{eq` →
  equation, eqnarray, eqnarray* (lines 126, 125, 124).
* `#INS#` once = cursor, twice = selection; `#RET#` = newline + current
  indentation; `#T#` = tab; `#INC#` (XML file) is treated as `#INS#`.
* Repeated Esc cycles through the candidates and then back to the typed word.
* Marks `•` / `•‹comment›`: Next/Previous Mark ⌃⌘F / ⌃⌘G, with ⌥ to delete
  the Mark, and Insert Mark ⌘8.

Checked in the running app: `benu`⎋ expands to an indented enumerate
environment with the Mark selected, and `\begin{eq`⎋⎋⎋⎋ cycles in the
order above and back to `\begin{eq`.

On macOS 26, Esc reaches NSTextView as `cancelOperation:`, which is not
forwarded to `complete:`. `RTTextView` routes it explicitly.
