//! LaTeX syntax tokenizer for the editor (TeXShop: `SyntaxHighlighting`
//! category on TSDocument; colour keys `commandred/green/blue`,
//! `commentred/...`, `markerred/...`, `indexred/...` in FactoryDefaults.plist).
//!
//! Spans are produced in UTF-16 code units so AppKit can use them directly.
//! Every token ends at or before a newline, so the editor can re-highlight
//! only the edited lines.

use serde::Serialize;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TokenKind {
    Command = 1,
    Comment = 2,
    Math = 3,
    Brace = 4,
    /// `%:` tag lines
    Marker = 5,
    /// `\index{...}` and `\label{...}`/`\ref{...}` arguments
    Index = 6,
    /// `\begin{..}`/`\end{..}` environment names
    Environment = 7,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: u32,
    pub len: u32,
    pub kind: u8,
}

/// Tokenize `text`; offsets are relative to `text` in UTF-16 units.
pub fn highlight(text: &str) -> Vec<Span> {
    let mut spans = Vec::with_capacity(text.len() / 8);
    let b = text.as_bytes();
    let mut i = 0usize; // byte index
    let mut u = 0u32; // utf-16 index
    let push = |spans: &mut Vec<Span>, start: u32, end: u32, kind: TokenKind| {
        if end > start {
            spans.push(Span { start, len: end - start, kind: kind as u8 });
        }
    };
    let mut line_start = true;
    while i < b.len() {
        let c = b[i];
        match c {
            b'%' => {
                let start = u;
                // `line_start` stays true across leading whitespace.
                let marker = line_start && b.get(i + 1) == Some(&b':');
                let (ni, nu) = advance_to_eol(text, i, u);
                push(&mut spans, start, nu, if marker { TokenKind::Marker } else { TokenKind::Comment });
                i = ni;
                u = nu;
                continue;
            }
            b'\\' => {
                let start = u;
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'@') {
                    j += 1;
                }
                if j == i + 1 && j < b.len() && b[j] != b'\n' {
                    // Control symbol: `\%`, `\\`, `\[`.
                    let ch = text[j..].chars().next().unwrap();
                    j += ch.len_utf8();
                    let end = start + 1 + ch.len_utf16() as u32;
                    let kind = if matches!(ch, '[' | ']' | '(' | ')') { TokenKind::Math } else { TokenKind::Command };
                    push(&mut spans, start, end, kind);
                    i = j;
                    u = end;
                    line_start = false;
                    continue;
                }
                let name = &text[i + 1..j];
                let end = start + (j - i) as u32;
                push(&mut spans, start, end, TokenKind::Command);
                i = j;
                u = end;
                // Highlight the argument of a few commands.
                let arg_kind = match name {
                    "begin" | "end" => Some(TokenKind::Environment),
                    "index" | "label" | "ref" | "eqref" | "pageref" | "cite" | "autoref" | "cref" => Some(TokenKind::Index),
                    _ => None,
                };
                if let (Some(kind), Some(b'{')) = (arg_kind, b.get(i)) {
                    if let Some(close) = text[i + 1..].find(['}', '\n']) {
                        if b[i + 1 + close] == b'}' {
                            push(&mut spans, u, u + 1, TokenKind::Brace);
                            let inner = &text[i + 1..i + 1 + close];
                            let inner16 = inner.encode_utf16().count() as u32;
                            push(&mut spans, u + 1, u + 1 + inner16, kind);
                            push(&mut spans, u + 1 + inner16, u + 2 + inner16, TokenKind::Brace);
                            i += close + 2;
                            u += inner16 + 2;
                        }
                    }
                }
                line_start = false;
                continue;
            }
            b'$' => {
                let n = if b.get(i + 1) == Some(&b'$') { 2 } else { 1 };
                push(&mut spans, u, u + n, TokenKind::Math);
                i += n as usize;
                u += n;
                line_start = false;
                continue;
            }
            b'{' | b'}' => {
                push(&mut spans, u, u + 1, TokenKind::Brace);
            }
            b'\n' => {
                line_start = true;
                i += 1;
                u += 1;
                continue;
            }
            b' ' | b'\t' => {}
            _ => line_start = false,
        }
        if c < 0x80 {
            i += 1;
            u += 1;
        } else {
            let ch = text[i..].chars().next().unwrap();
            i += ch.len_utf8();
            u += ch.len_utf16() as u32;
            line_start = false;
        }
    }
    spans
}

fn advance_to_eol(text: &str, i: usize, u: u32) -> (usize, u32) {
    let rest = &text[i..];
    let end = rest.find('\n').unwrap_or(rest.len());
    (i + end, u + rest[..end].encode_utf16().count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(String, u8)> {
        let u: Vec<u16> = s.encode_utf16().collect();
        highlight(s)
            .iter()
            .map(|sp| (String::from_utf16(&u[sp.start as usize..(sp.start + sp.len) as usize]).unwrap(), sp.kind))
            .collect()
    }

    #[test]
    fn basic_tokens() {
        let k = kinds("\\section{Héllo} $x$ % note\n%: mark\n\\begin{eq}\\%");
        assert_eq!(
            k,
            vec![
                ("\\section".into(), 1),
                ("{".into(), 4),
                ("}".into(), 4),
                ("$".into(), 3),
                ("$".into(), 3),
                ("% note".into(), 2),
                ("%: mark".into(), 5),
                ("\\begin".into(), 1),
                ("{".into(), 4),
                ("eq".into(), 7),
                ("}".into(), 4),
                ("\\%".into(), 1),
            ]
        );
    }

    #[test]
    fn utf16_offsets_with_astral_chars() {
        let s = "😀 \\cmd";
        let sp = highlight(s);
        assert_eq!(sp[0].start, 3); // emoji = 2 units, then space
        assert_eq!(sp[0].len, 4);
    }
}
