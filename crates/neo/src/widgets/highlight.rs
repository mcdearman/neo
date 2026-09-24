//! A small, fast syntax highlighter for the code editor. It works one line
//! at a time and carries only block-comment state between lines.

use neo_theme::{Accent, Color, Palette, Scheme};

/// Languages the editor can colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    #[default]
    Plain,
    Rust,
    Toml,
    Markdown,
}

impl Language {
    /// Guesses from a file name.
    pub fn from_path(path: &str) -> Self {
        match path.rsplit('.').next().unwrap_or("") {
            "rs" => Language::Rust,
            "toml" => Language::Toml,
            "md" | "markdown" => Language::Markdown,
            _ => Language::Plain,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Plain => "Plain Text",
            Language::Rust => "Rust",
            Language::Toml => "TOML",
            Language::Markdown => "Markdown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Plain,
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Macro,
    Attribute,
    Punct,
    Heading,
}

/// Colours for each token kind, derived from the theme.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SyntaxColors {
    keyword: Color,
    ty: Color,
    function: Color,
    string: Color,
    number: Color,
    comment: Color,
    macro_: Color,
    punct: Color,
}

impl SyntaxColors {
    pub fn new(p: &Palette, scheme: Scheme, accent: Accent) -> Self {
        // Types use teal, unless teal is the accent, in which case royal blue.
        let ty = if accent == Accent::Teal { Accent::Royal } else { Accent::Teal };
        Self {
            keyword: p.accent_text,
            ty: ty.text_tone(scheme),
            function: Accent::Amber.text_tone(scheme),
            string: p.good,
            number: Accent::Coral.text_tone(scheme),
            comment: p.muted,
            macro_: Accent::Coral.text_tone(scheme),
            punct: p.muted,
        }
    }

    pub fn color(&self, k: Kind) -> Option<Color> {
        match k {
            Kind::Plain => None,
            Kind::Keyword | Kind::Heading => Some(self.keyword),
            Kind::Type => Some(self.ty),
            Kind::Function => Some(self.function),
            Kind::String => Some(self.string),
            Kind::Number => Some(self.number),
            Kind::Comment => Some(self.comment),
            Kind::Macro | Kind::Attribute => Some(self.macro_),
            Kind::Punct => Some(self.punct),
        }
    }
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
    "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "yield",
];

const RUST_PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64",
];

/// Splits `line` into highlighted byte ranges. `in_comment` carries Rust
/// block-comment state across lines.
pub(crate) fn highlight(lang: Language, line: &str, in_comment: &mut bool) -> Vec<(usize, usize, Kind)> {
    match lang {
        Language::Plain => vec![(0, line.len(), Kind::Plain)],
        Language::Rust => rust(line, in_comment),
        Language::Toml => toml(line),
        Language::Markdown => markdown(line),
    }
}

fn push(out: &mut Vec<(usize, usize, Kind)>, a: usize, b: usize, k: Kind) {
    if b <= a {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.2 == k && last.1 == a {
            last.1 = b;
            return;
        }
    out.push((a, b, k));
}

fn ident_end(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'_' || s[i] >= 0x80) {
        i += 1;
    }
    i
}

fn rust(line: &str, in_comment: &mut bool) -> Vec<(usize, usize, Kind)> {
    let s = line.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < s.len() {
        if *in_comment {
            match line[i..].find("*/") {
                Some(e) => {
                    push(&mut out, i, i + e + 2, Kind::Comment);
                    i += e + 2;
                    *in_comment = false;
                }
                None => {
                    push(&mut out, i, s.len(), Kind::Comment);
                    i = s.len();
                }
            }
            continue;
        }
        let c = s[i];
        let rest = &line[i..];
        if rest.starts_with("//") {
            push(&mut out, i, s.len(), Kind::Comment);
            break;
        }
        if rest.starts_with("/*") {
            *in_comment = true;
            push(&mut out, i, i + 2, Kind::Comment);
            i += 2;
            continue;
        }
        if c == b'"' {
            let mut j = i + 1;
            while j < s.len() && s[j] != b'"' {
                j += if s[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(s.len());
            push(&mut out, i, end, Kind::String);
            i = end;
            continue;
        }
        if c == b'\'' {
            // Char literal ('a', '\n') or lifetime ('a).
            let is_char = (s.len() > i + 2 && s[i + 2] == b'\'') || (s.len() > i + 3 && s[i + 1] == b'\\' && s[i + 3] == b'\'');
            if is_char {
                let end = if s[i + 1] == b'\\' { i + 4 } else { i + 3 };
                push(&mut out, i, end, Kind::String);
                i = end;
            } else {
                let end = ident_end(s, i + 1);
                push(&mut out, i, end.max(i + 1), Kind::Type);
                i = end.max(i + 1);
            }
            continue;
        }
        if c == b'#' && (rest.starts_with("#[") || rest.starts_with("#![")) {
            let end = rest.find(']').map_or(s.len(), |e| i + e + 1);
            push(&mut out, i, end, Kind::Attribute);
            i = end;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < s.len() && (s[j].is_ascii_alphanumeric() || s[j] == b'_' || (s[j] == b'.' && j + 1 < s.len() && s[j + 1].is_ascii_digit())) {
                j += 1;
            }
            push(&mut out, i, j, Kind::Number);
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let end = ident_end(s, i);
            let word = &line[i..end];
            let next = line[end..].trim_start().as_bytes().first().copied();
            let kind = if RUST_KEYWORDS.contains(&word) {
                Kind::Keyword
            } else if next == Some(b'!') && !line[end..].starts_with("!=") {
                Kind::Macro
            } else if RUST_PRIMITIVES.contains(&word) || word.starts_with(|c: char| c.is_ascii_uppercase()) {
                Kind::Type
            } else if next == Some(b'(') || line[end..].starts_with("::<") {
                Kind::Function
            } else {
                Kind::Plain
            };
            let end = if kind == Kind::Macro { end + 1 } else { end };
            push(&mut out, i, end.min(s.len()), kind);
            i = end.min(s.len());
            continue;
        }
        if c.is_ascii_whitespace() {
            push(&mut out, i, i + 1, Kind::Plain);
            i += 1;
            continue;
        }
        let len = line[i..].chars().next().map_or(1, char::len_utf8);
        push(&mut out, i, i + len, Kind::Punct);
        i += len;
    }
    out
}

fn toml(line: &str) -> Vec<(usize, usize, Kind)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let mut out = vec![];
    push(&mut out, 0, indent, Kind::Plain);
    if trimmed.starts_with('#') {
        push(&mut out, indent, line.len(), Kind::Comment);
    } else if trimmed.starts_with('[') {
        push(&mut out, indent, line.len(), Kind::Heading);
    } else if let Some(eq) = line.find('=') {
        push(&mut out, indent, eq, Kind::Function);
        push(&mut out, eq, eq + 1, Kind::Punct);
        let value = &line[eq + 1..];
        let v = value.trim_start();
        let start = eq + 1 + (value.len() - v.len());
        push(&mut out, eq + 1, start, Kind::Plain);
        let kind = if v.starts_with(['"', '\'']) {
            Kind::String
        } else if v.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
            Kind::Number
        } else if v.starts_with("true") || v.starts_with("false") {
            Kind::Keyword
        } else {
            Kind::Plain
        };
        // Inline tables and arrays: colour strings inside, keep the rest plain.
        if kind == Kind::Plain {
            let mut in_str = false;
            let mut from = start;
            for (off, ch) in line[start..].char_indices() {
                let at = start + off;
                if ch == '"' {
                    if in_str {
                        push(&mut out, from, at + 1, Kind::String);
                        from = at + 1;
                    } else {
                        push(&mut out, from, at, Kind::Plain);
                        from = at;
                    }
                    in_str = !in_str;
                }
            }
            push(&mut out, from, line.len(), if in_str { Kind::String } else { Kind::Plain });
        } else {
            push(&mut out, start, line.len(), kind);
        }
    } else {
        push(&mut out, indent, line.len(), Kind::Plain);
    }
    out
}

fn markdown(line: &str) -> Vec<(usize, usize, Kind)> {
    let mut out = vec![];
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        push(&mut out, 0, line.len(), Kind::Heading);
        return out;
    }
    if trimmed.starts_with("```") {
        push(&mut out, 0, line.len(), Kind::Comment);
        return out;
    }
    let lead = line.len() - trimmed.len();
    push(&mut out, 0, lead, Kind::Plain);
    let mut i = lead;
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("> ") {
        push(&mut out, i, i + 1, Kind::Punct);
        i += 1;
    }
    let s = line.as_bytes();
    while i < s.len() {
        if s[i] == b'`' {
            let end = line[i + 1..].find('`').map_or(s.len(), |e| i + e + 2);
            push(&mut out, i, end, Kind::String);
            i = end;
        } else if s[i] == b'[' {
            let end = line[i..].find(')').filter(|_| line[i..].contains("](")).map_or(i + 1, |e| i + e + 1);
            push(&mut out, i, end, if end > i + 1 { Kind::Type } else { Kind::Plain });
            i = end;
        } else if line[i..].starts_with("**") {
            let end = line[i + 2..].find("**").map_or(s.len(), |e| i + e + 4);
            push(&mut out, i, end, Kind::Keyword);
            i = end;
        } else {
            let len = line[i..].chars().next().map_or(1, char::len_utf8);
            push(&mut out, i, i + len, Kind::Plain);
            i += len;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<(&str, Kind)> {
        let mut c = false;
        highlight(Language::Rust, line, &mut c).into_iter().filter(|t| t.2 != Kind::Plain).map(|(a, b, k)| (&line[a..b], k)).collect()
    }

    #[test]
    fn rust_tokens() {
        let k = kinds(r#"pub fn main() -> Result<u8, E> { println!("hi {}", 42); } // done"#);
        assert!(k.contains(&("pub", Kind::Keyword)));
        assert!(k.contains(&("main", Kind::Function)));
        assert!(k.contains(&("Result", Kind::Type)));
        assert!(k.contains(&("u8", Kind::Type)));
        assert!(k.contains(&("println!", Kind::Macro)));
        assert!(k.contains(&(r#""hi {}""#, Kind::String)));
        assert!(k.contains(&("42", Kind::Number)));
        assert!(k.contains(&("// done", Kind::Comment)));
    }

    #[test]
    fn block_comments_span_lines() {
        let mut c = false;
        let a = highlight(Language::Rust, "let a = 1; /* start", &mut c);
        assert!(c);
        assert_eq!(a.last().unwrap().2, Kind::Comment);
        let b = highlight(Language::Rust, "still */ let", &mut c);
        assert!(!c);
        assert_eq!(b[0], (0, 8, Kind::Comment));
    }

    #[test]
    fn lifetimes_and_chars() {
        let k = kinds("fn f<'a>(x: &'a str) -> char { 'x' }");
        assert!(k.contains(&("'a", Kind::Type)));
        assert!(k.contains(&("'x'", Kind::String)));
    }

    #[test]
    fn spans_cover_the_whole_line() {
        for line in ["", "  let x = \"a\\\"b\";", "#[derive(Debug)]", "é = 1 // ünïcode", "[package]", "name = \"neo\""] {
            for lang in [Language::Rust, Language::Toml, Language::Markdown, Language::Plain] {
                let mut c = false;
                let spans = highlight(lang, line, &mut c);
                let mut at = 0;
                for (a, b, _) in &spans {
                    assert_eq!(*a, at, "{lang:?} gap in {line:?}");
                    at = *b;
                }
                assert_eq!(at, line.len(), "{lang:?} does not cover {line:?}");
            }
        }
    }
}
