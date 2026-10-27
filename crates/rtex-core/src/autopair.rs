//! Auto-pairing, recovered from TeXShop 5.57 with REA + Ghidra (see
//! docs/RE-NOTES.md, "Auto-pairing"):
//!
//! * `-[TSTextView insertText:]` looks a typed character up in the
//!   "autocompletion" dictionary (`Keyboard/autocompletion.plist`) and hands
//!   the value to `-[TSDocument insertSpecialNonStandard:undoKey:]`, which
//!   replaces `#SEL#` with the selected text, removes the *last* `#INS#`
//!   (backwards search) and puts the caret there. ASCII keys typed right
//!   after a backslash are left alone (`\^`, `\_`, `\"` stay literal).
//! * `-[TSTextView keyDown:]` with Esc right after `\begin{name}` inserts
//!   `"\n" + indent + "\end{name}\n"` and leaves the caret where it was.
//!
//! RTeX additions (not in TeXShop): built-in pairs for `{ [ (`, typing over
//! an auto-inserted closer, and Backspace removing an empty pair. Those live
//! in the app; this module holds the text rules.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Insertion {
    pub text: String,
    /// Caret offset in UTF-16 units from the start of `text`.
    pub caret: u32,
}

/// Expand a TeXShop autocompletion template with the current selection.
pub fn expand(template: &str, selected: &str) -> Insertion {
    let with_sel = template.replace("#SEL#", selected);
    match with_sel.rfind("#INS#") {
        Some(i) => {
            let text = format!("{}{}", &with_sel[..i], &with_sel[i + 5..]);
            Insertion { caret: with_sel[..i].encode_utf16().count() as u32, text }
        }
        None => Insertion { caret: with_sel.encode_utf16().count() as u32, text: with_sel },
    }
}

/// RTeX's defaults, used for keys the loaded table does not define.
pub const DEFAULT_PAIRS: &[(&str, &str)] = &[("{", "{#SEL##INS#}"), ("[", "[#SEL##INS#]"), ("(", "(#SEL##INS#)")];

/// Closing character for an opener RTeX pairs.
pub fn closer_for(opener: char) -> Option<char> {
    match opener {
        '{' => Some('}'),
        '[' => Some(']'),
        '(' => Some(')'),
        _ => None,
    }
}

/// Esc after `\begin{name}`: the text TeXShop inserts at the cursor, given
/// the line up to the cursor and its indentation. None when the cursor is not
/// right after a `\begin{...}` group.
pub fn end_environment(before_cursor: &str, indent: &str) -> Option<String> {
    let rest = before_cursor.strip_suffix('}')?;
    let open = rest.rfind('{')?;
    let name = &rest[open + 1..];
    if name.is_empty() || name.contains(['{', '}', '\n']) || !rest[..open].ends_with("\\begin") {
        return None;
    }
    Some(format!("\n{indent}\\end{{{name}}}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texshop_templates() {
        assert_eq!(expand("^{#SEL##INS#}", ""), Insertion { text: "^{}".into(), caret: 2 });
        assert_eq!(expand("^{#SEL##INS#}", "n+1"), Insertion { text: "^{n+1}".into(), caret: 5 });
        assert_eq!(expand("``#SEL##INS#''", "quote"), Insertion { text: "``quote''".into(), caret: 7 });
        assert_eq!(expand("\\leq", ""), Insertion { text: "\\leq".into(), caret: 4 });
        // Last #INS# wins (backwards search), earlier ones stay as text.
        assert_eq!(expand("a#INS#b#INS#c", "").text, "a#INS#bc");
    }

    #[test]
    fn utf16_caret() {
        assert_eq!(expand("😀{#INS#}", "").caret, 3);
    }

    #[test]
    fn end_environment_rules() {
        assert_eq!(end_environment("  \\begin{itemize}", "  ").as_deref(), Some("\n  \\end{itemize}\n"));
        assert_eq!(end_environment("\\begin{align*}", "").as_deref(), Some("\n\\end{align*}\n"));
        assert_eq!(end_environment("\\section{Intro}", ""), None);
        assert_eq!(end_environment("\\begin{}", ""), None);
        assert_eq!(end_environment("\\begin{x} more", ""), None);
    }

    #[test]
    fn defaults_cover_brackets() {
        for (k, v) in DEFAULT_PAIRS {
            let c = k.chars().next().unwrap();
            assert!(v.ends_with(closer_for(c).unwrap()));
        }
    }
}
