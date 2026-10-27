// C interface to rtex-core (crates/rtex-core/src/ffi.rs).
//
// All strings are NUL-terminated UTF-8. Every char* returned must be freed
// with rtex_string_free. JSON results either hold the documented object or
// {"error": "..."}.
#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct rtex_cancel rtex_cancel;  // opaque cancellation token

typedef struct {
    uint32_t start;  // UTF-16 offset
    uint32_t len;    // UTF-16 length
    uint8_t kind;    // see RTEX_TOKEN_*
} rtex_span;

enum {
    RTEX_TOKEN_COMMAND = 1,
    RTEX_TOKEN_COMMENT = 2,
    RTEX_TOKEN_MATH = 3,
    RTEX_TOKEN_BRACE = 4,
    RTEX_TOKEN_MARKER = 5,
    RTEX_TOKEN_INDEX = 6,
    RTEX_TOKEN_ENVIRONMENT = 7,
};

typedef void (*rtex_output_fn)(void *ctx, const char *chunk);

void rtex_string_free(char *s);

rtex_cancel *rtex_cancel_new(void);
void rtex_cancel_set(rtex_cancel *c);
void rtex_cancel_free(rtex_cancel *c);

// Typeset (blocking; call off the main thread). options_json: BuildOptions
// fields, e.g. {"engine":"xelatex","fast_preamble":true,"after_build":"hide"}. Returns a
// BuildReport: {root, engine, success, cancelled, pdf, pages, passes, steps,
// log:{messages:[{level,file,line,message,context}], ...}, error, total_millis}.
char *rtex_build(const char *path, const char *options_json, rtex_output_fn output, void *ctx,
                 const rtex_cancel *cancel);

// SyncTeX. Coordinates are PDF points, origin at the page's top-left.
char *rtex_synctex_forward(const char *pdf, const char *source, uint32_t line, uint32_t page_hint);
char *rtex_synctex_inverse(const char *pdf, uint32_t page, double x, double y);

char *rtex_outline(const char *text);     // [{kind, level, title, line, offset16}]
char *rtex_directives(const char *text);  // {root, program, encoding, ...}
char *rtex_parse_log(const char *text);
char *rtex_clean(const char *path, bool aggressive);
// Build files of the root currently in the folder (JSON array of paths).
char *rtex_build_files(const char *path);
char *rtex_resolve_root(const char *path);  // {root, directives}
char *rtex_engines(void);                   // [{name, kind, path}]

// Command completion (TeXShop CommandCompletion.txt format). `word` is the
// text left of the cursor back to a boundary character (a leading \ or { is
// included); `indent` the current line's leading whitespace. Returns
// {"source": path, "candidates": [{text, sel_start, sel_len}]}, best first;
// selection offsets are UTF-16 units into `text`.
char *rtex_complete(const char *word, const char *indent);

// Auto-pairing (TeXShop Keyboard/autocompletion.plist templates).
char *rtex_autopair_expand(const char *tmpl, const char *selected);  // {text, caret}
char *rtex_autopair_defaults(void);                                  // {key: template}
// Esc after \begin{name}: text to insert at the cursor, or NULL.
char *rtex_end_environment(const char *before_cursor, const char *indent);

// Files \input/\include/\subfile'd by a root's text, as a JSON array of
// absolute paths (relative names resolve against `dir`).
char *rtex_included_files(const char *text, const char *dir);

rtex_span *rtex_highlight(const uint8_t *text, size_t len, size_t *count);
void rtex_spans_free(rtex_span *spans, size_t count);

#ifdef __cplusplus
}
#endif
