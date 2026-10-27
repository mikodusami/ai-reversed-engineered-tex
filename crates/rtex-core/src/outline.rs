//! Tags / labels outline (TeXShop: the "Tags" and "Labels" toolbar menus,
//! `-[TSDocument doTag:]`, `doLabel:`, and the `%:` user-mark convention).

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Part,
    Chapter,
    Section,
    Subsection,
    Subsubsection,
    Paragraph,
    /// `%: My mark` (TeXShop user tag)
    Mark,
    Label,
    /// `\begin{frame}{Title}` (beamer)
    Frame,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub kind: EntryKind,
    /// 0 = part ... 5 = paragraph; marks and labels get 6.
    pub level: u8,
    pub title: String,
    /// 1-based line number.
    pub line: u32,
    /// UTF-16 offset of the line start (for NSTextView).
    pub offset16: usize,
}

const HEADINGS: &[(&str, EntryKind, u8)] = &[
    ("part", EntryKind::Part, 0),
    ("chapter", EntryKind::Chapter, 1),
    ("section", EntryKind::Section, 2),
    ("subsection", EntryKind::Subsection, 3),
    ("subsubsection", EntryKind::Subsubsection, 4),
    ("paragraph", EntryKind::Paragraph, 5),
];

pub fn outline(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut off16 = 0usize;
    for (i, line) in text.split('\n').enumerate() {
        let lno = i as u32 + 1;
        let t = line.trim_start();
        if let Some(mark) = t.strip_prefix("%:") {
            out.push(Entry { kind: EntryKind::Mark, level: 6, title: mark.trim().to_string(), line: lno, offset16: off16 });
        } else {
            let code = strip_comment(t);
            scan_line(code, lno, off16, &mut out);
        }
        off16 += line.encode_utf16().count() + 1;
    }
    out
}

fn scan_line(code: &str, lno: u32, off16: usize, out: &mut Vec<Entry>) {
    let mut rest = code;
    while let Some(p) = rest.find('\\') {
        rest = &rest[p + 1..];
        let name_len = rest.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
        let name = &rest[..name_len];
        let mut after = &rest[name_len..];
        if let Some((_, kind, level)) = HEADINGS.iter().find(|(n, ..)| *n == name) {
            after = after.strip_prefix('*').unwrap_or(after);
            let after = skip_optional(after.trim_start());
            if let Some(title) = braced(after) {
                out.push(Entry { kind: kind.clone(), level: *level, title: clean(title), line: lno, offset16: off16 });
            }
        } else if name == "label" {
            if let Some(title) = braced(after.trim_start()) {
                out.push(Entry { kind: EntryKind::Label, level: 6, title: title.to_string(), line: lno, offset16: off16 });
            }
        } else if name == "begin" && after.starts_with("{frame}") {
            let after = skip_optional(after["{frame}".len()..].trim_start());
            let title = braced(after).map(clean).unwrap_or_else(|| "frame".into());
            out.push(Entry { kind: EntryKind::Frame, level: 3, title, line: lno, offset16: off16 });
        }
        // Skip the command name, or a control symbol's single char.
        let skip = if name_len > 0 { name_len } else { rest.chars().next().map_or(0, char::len_utf8) };
        rest = &rest[skip..];
    }
}

/// Text up to an unescaped `%`.
pub fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'%' => return &line[..i],
            _ => i += 1,
        }
    }
    line
}

fn skip_optional(s: &str) -> &str {
    if !s.starts_with('[') {
        return s;
    }
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return s[i + 1..].trim_start();
                }
            }
            _ => {}
        }
    }
    s
}

/// Contents of a balanced `{...}` group at the start of `s`.
fn braced(s: &str) -> Option<&str> {
    let s = s.strip_prefix('{')?;
    let mut depth = 1;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    // Unbalanced on this line: take what is there.
    Some(s)
}

fn clean(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_headings_labels_marks() {
        let src = "\\documentclass{book}\n%: Draft notes\n\\chapter*{Intro}\\label{ch:intro}\n% \\section{Commented}\n\\section[short]{A {nested} title}\n\\subsection{Ünïcode}\n\\begin{frame}{Slide}\n";
        let o = outline(src);
        let titles: Vec<_> = o.iter().map(|e| (e.kind.clone(), e.title.as_str(), e.line)).collect();
        assert_eq!(
            titles,
            vec![
                (EntryKind::Mark, "Draft notes", 2),
                (EntryKind::Chapter, "Intro", 3),
                (EntryKind::Label, "ch:intro", 3),
                (EntryKind::Section, "A {nested} title", 5),
                (EntryKind::Subsection, "Ünïcode", 6),
                (EntryKind::Frame, "Slide", 7),
            ]
        );
        assert_eq!(o[1].offset16, "\\documentclass{book}\n%: Draft notes\n".len());
    }

    #[test]
    fn escaped_percent_is_not_comment() {
        assert_eq!(strip_comment("50\\% done % note"), "50\\% done ");
    }
}
