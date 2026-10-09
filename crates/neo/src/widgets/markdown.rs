//! Prose written as Markdown, shown as what it means: headings, lists,
//! words in bold and italic, code in the fixed-width face with its
//! colours, and links to follow.
//!
//! It is for what an assistant writes, which arrives a little at a time:
//! whatever is not finished yet, a fence not closed or stars not matched,
//! is shown as far as it goes and not as a mistake.

use armature_render::{Point, Rect, Size, Span, TextLayout, TextStyle};
use neo_theme::{Surface, TextRole};

use super::highlight::{highlight, Language, SyntaxColors};
use crate::ThemeCx;
use crate::core::{Cx, CursorIcon, DrawCx, Element, EventCx, Length, Limits, Widget};
use crate::event::{Event, PointerButton, Status};

/// A stretch of prose in one manner.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// Where it leads, if it is a link.
    pub link: Option<String>,
}

/// What kind of paragraph a piece of prose is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Paragraph,
    /// A heading, from 1, the largest, to 6.
    Heading(u8),
    /// Something quoted.
    Quote,
}

/// A piece of a Markdown text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// Prose: a paragraph, a heading, or an item of a list, which has the
    /// mark it goes by (`•`, `2.`) and how far in it is.
    Prose { kind: Kind, mark: Option<String>, depth: usize, inlines: Vec<Inline> },
    /// Code, set off on its own, in the language named if one was.
    Code { language: String, text: String },
    /// A line across.
    Rule,
}

/// Reads the prose of one paragraph: its stars, backticks and links.
pub fn inlines(text: &str) -> Vec<Inline> {
    let mut out: Vec<Inline> = vec![];
    let (mut bold, mut italic) = (false, false);
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<Inline>, bold: bool, italic: bool| {
        if !plain.is_empty() {
            out.push(Inline { text: std::mem::take(plain), bold, italic, ..Inline::default() });
        }
    };
    let chars: Vec<char> = text.chars().collect();
    let rest = |i: usize| chars[i..].iter().collect::<String>();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            // A mark with a backslash before it is only itself.
            '\\' if chars.get(i + 1).is_some_and(|n| "\\`*_[]()#>-".contains(*n)) => {
                plain.push(chars[i + 1]);
                i += 2;
            }
            '`' => match rest(i + 1).find('`') {
                Some(end) => {
                    flush(&mut plain, &mut out, bold, italic);
                    let code: String = rest(i + 1).chars().take(rest(i + 1)[..end].chars().count()).collect();
                    i += 2 + code.chars().count();
                    out.push(Inline { text: code, bold, italic, code: true, link: None });
                }
                // Not closed yet: a backtick, until the rest arrives.
                None => {
                    plain.push(c);
                    i += 1;
                }
            },
            '*' | '_' => {
                let double = chars.get(i + 1) == Some(&c);
                let width = if double { 2 } else { 1 };
                // An underscore inside a word is part of the word.
                let in_word = c == '_' && i > 0 && chars[i - 1].is_alphanumeric() && chars.get(i + width).is_some_and(|n| n.is_alphanumeric());
                let on = if double { bold } else { italic };
                let mark: String = std::iter::repeat_n(c, width).collect();
                // Opened only if it is closed further on, and not before a space.
                let opens = !on && chars.get(i + width).is_some_and(|n| !n.is_whitespace()) && rest(i + width).contains(&mark);
                if in_word || !(on || opens) {
                    plain.push_str(&mark);
                } else {
                    flush(&mut plain, &mut out, bold, italic);
                    if double { bold = !bold } else { italic = !italic }
                }
                i += width;
            }
            '[' => {
                let after = rest(i + 1);
                let link = after.find("](").and_then(|close| after[close + 2..].find(')').map(|end| (after[..close].to_owned(), after[close + 2..close + 2 + end].to_owned())));
                match link.filter(|(label, to)| !label.contains('[') && !to.contains(char::is_whitespace)) {
                    Some((label, to)) => {
                        flush(&mut plain, &mut out, bold, italic);
                        i += label.chars().count() + to.chars().count() + 4;
                        out.push(Inline { text: label, bold, italic, code: false, link: Some(to) });
                    }
                    None => {
                        plain.push(c);
                        i += 1;
                    }
                }
            }
            // An address written out is a link to itself.
            'h' if (i == 0 || !chars[i - 1].is_alphanumeric()) && (rest(i).starts_with("http://") || rest(i).starts_with("https://")) => {
                let address: String = chars[i..].iter().take_while(|c| !c.is_whitespace() && !"<>\"".contains(**c)).collect();
                let address = address.trim_end_matches(['.', ',', ')', ';', ':', '!', '?']).to_owned();
                flush(&mut plain, &mut out, bold, italic);
                i += address.chars().count();
                out.push(Inline { text: address.clone(), bold, italic, code: false, link: Some(address) });
            }
            _ => {
                plain.push(c);
                i += 1;
            }
        }
    }
    flush(&mut plain, &mut out, bold, italic);
    out
}

/// Reads a Markdown text into its pieces.
pub fn blocks(text: &str) -> Vec<Block> {
    let mut out = vec![];
    // The paragraph being gathered: its kind, mark and depth, and its lines.
    let mut open: Option<(Kind, Option<String>, usize, String)> = None;
    let close = |open: &mut Option<(Kind, Option<String>, usize, String)>, out: &mut Vec<Block>| {
        if let Some((kind, mark, depth, text)) = open.take() {
            out.push(Block::Prose { kind, mark, depth, inlines: inlines(text.trim()) });
        }
    };
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(fence) = trimmed.strip_prefix("```") {
            close(&mut open, &mut out);
            // To the closing fence, or as far as there is yet.
            let code: Vec<&str> = lines.by_ref().take_while(|l| !l.trim_start().starts_with("```")).collect();
            out.push(Block::Code { language: fence.trim().to_owned(), text: code.join("\n") });
            continue;
        }
        if trimmed.is_empty() {
            close(&mut open, &mut out);
            continue;
        }
        if trimmed.len() >= 3 && (trimmed.chars().all(|c| c == '-') || trimmed.chars().all(|c| c == '*') || trimmed.chars().all(|c| c == '_')) {
            close(&mut open, &mut out);
            out.push(Block::Rule);
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            close(&mut open, &mut out);
            out.push(Block::Prose { kind: Kind::Heading(hashes as u8), mark: None, depth: 0, inlines: inlines(trimmed[hashes..].trim().trim_end_matches('#').trim_end()) });
            continue;
        }
        // An item of a list: a dash, star or plus, or a number and a point.
        let bullet = ["- ", "* ", "+ "].iter().find_map(|b| trimmed.strip_prefix(b).map(|rest| ("•".to_owned(), rest)));
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        let numbered = (digits > 0 && digits <= 9).then(|| trimmed[digits..].strip_prefix(". ").or_else(|| trimmed[digits..].strip_prefix(") ")).map(|rest| (format!("{}.", &trimmed[..digits]), rest))).flatten();
        if let Some((mark, rest)) = bullet.or(numbered) {
            close(&mut open, &mut out);
            open = Some((Kind::Paragraph, Some(mark), indent / 2, rest.to_owned()));
            continue;
        }
        if let Some(quoted) = trimmed.strip_prefix('>') {
            match &mut open {
                Some((Kind::Quote, _, _, text)) => {
                    text.push(' ');
                    text.push_str(quoted.trim());
                }
                _ => {
                    close(&mut open, &mut out);
                    open = Some((Kind::Quote, None, 0, quoted.trim().to_owned()));
                }
            }
            continue;
        }
        // More of the paragraph, or the item, before it; or a new paragraph.
        match &mut open {
            Some((_, _, _, text)) => {
                text.push(' ');
                text.push_str(trimmed);
            }
            None => open = Some((Kind::Paragraph, None, 0, trimmed.to_owned())),
        }
    }
    close(&mut open, &mut out);
    out
}

/// A piece as it is laid out.
struct Laid {
    /// Its place, from the widget's own corner.
    rect: Rect,
    kind: LaidKind,
}

enum LaidKind {
    Prose { text: TextLayout, mark: Option<TextLayout>, quote: bool, links: Vec<(std::ops::Range<usize>, String)> },
    Code { lines: Vec<TextLayout>, line: f32 },
    Rule,
}

/// Shows Markdown: see [`markdown`].
pub struct Markdown<M> {
    blocks: Vec<Block>,
    on_link: Option<Box<dyn Fn(String) -> M>>,
    laid: Vec<Laid>,
}

/// `text`, read as Markdown and shown as what it means, as wide as it is
/// let be and as tall as that makes it.
pub fn markdown<M>(text: &str) -> Markdown<M> {
    Markdown { blocks: blocks(text), on_link: None, laid: vec![] }
}

impl<M> Markdown<M> {
    /// Told the address of a link that is clicked. What following it
    /// means is the app's to say.
    pub fn on_link(mut self, f: impl Fn(String) -> M + 'static) -> Self {
        self.on_link = Some(Box::new(f));
        self
    }

    fn link_at(&self, origin: Point, p: Point) -> Option<&str> {
        self.laid.iter().find_map(|l| {
            let LaidKind::Prose { text, links, .. } = &l.kind else { return None };
            let at = text.index_at(Point::new(p.x - origin.x - l.rect.x, p.y - origin.y - l.rect.y))?;
            links.iter().find(|(range, _)| range.contains(&at)).map(|(_, to)| to.as_str())
        })
    }
}

const GAP: f32 = 10.0;
const INDENT: f32 = 20.0;
const CODE_PAD: f32 = 10.0;

impl<M: 'static> Widget<M> for Markdown<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let width = limits.constrain(Length::Fill, Length::Shrink).max.w;
        let theme = *cx.theme();
        let p = theme.palette();
        let colors = SyntaxColors::new(&p, theme.scheme, theme.accent, theme.syntax);
        self.laid.clear();
        let mut y = 0.0;
        for (n, block) in self.blocks.iter().enumerate() {
            if n > 0 {
                // An item sits close under the item before it.
                let close = matches!((&self.blocks[n - 1], block), (Block::Prose { mark: Some(_), .. }, Block::Prose { mark: Some(_), .. }));
                y += if close { 4.0 } else { GAP };
            }
            match block {
                Block::Prose { kind, mark, depth, inlines } => {
                    let style = match kind {
                        Kind::Heading(1) => theme.text(TextRole::Heading).style(),
                        Kind::Heading(2) => theme.text(TextRole::Title).style(),
                        Kind::Heading(_) => theme.text(TextRole::Strong).style(),
                        _ => theme.text(TextRole::Body).style(),
                    };
                    let x = *depth as f32 * INDENT + if mark.is_some() { INDENT } else { 0.0 } + if *kind == Kind::Quote { 14.0 } else { 0.0 };
                    let spans: Vec<Span> = inlines.iter().map(|i| Span { text: &i.text, color: if i.link.is_some() { Some(p.accent_text) } else if i.code { Some(p.muted) } else { None }, bold: i.bold, italic: i.italic || *kind == Kind::Quote, mono: i.code }).collect();
                    let text = cx.text().layout_rich(&spans, &style, Some((width - x).max(40.0)));
                    let mut at = 0;
                    let links = inlines
                        .iter()
                        .filter_map(|i| {
                            let range = at..at + i.text.len();
                            at = range.end;
                            i.link.clone().map(|to| (range, to))
                        })
                        .collect();
                    let mark = mark.as_ref().map(|m| cx.text().layout(m, &style, None));
                    let h = text.size().h;
                    self.laid.push(Laid { rect: Rect::new(x, y, (width - x).max(0.0), h), kind: LaidKind::Prose { text, mark, quote: *kind == Kind::Quote, links } });
                    y += h;
                }
                Block::Code { language, text } => {
                    let style = TextStyle { size: 12.5 * theme.text_scale, weight: 400, family: armature_render::FontFamily::Mono, line_height: 1.5, letter_spacing: 0.0 };
                    let language = Language::from_name(language).unwrap_or(Language::Plain);
                    let mut in_comment = false;
                    let lines: Vec<TextLayout> = text
                        .lines()
                        .map(|line| {
                            let spans: Vec<(&str, Option<armature_render::Color>)> = highlight(language, line, &mut in_comment).into_iter().map(|(a, b, k)| (&line[a..b], colors.color(k))).collect();
                            if spans.is_empty() { cx.text().layout(line, &style, None) } else { cx.text().layout_spans(&spans, &style, None) }
                        })
                        .collect();
                    let line = (style.size * style.line_height).round();
                    let h = lines.len().max(1) as f32 * line + CODE_PAD * 2.0;
                    self.laid.push(Laid { rect: Rect::new(0.0, y, width, h), kind: LaidKind::Code { lines, line } });
                    y += h;
                }
                Block::Rule => {
                    self.laid.push(Laid { rect: Rect::new(0.0, y + 4.0, width, 1.0), kind: LaidKind::Rule });
                    y += 9.0;
                }
            }
        }
        Size::new(width, y)
    }

    fn draw(&self, cx: &mut DrawCx) {
        let origin = cx.bounds().origin();
        let theme = *cx.theme();
        let p = theme.palette();
        let content = cx.content_color();
        for l in &self.laid {
            let r = l.rect.translate(origin);
            match &l.kind {
                LaidKind::Prose { text, mark, quote, .. } => {
                    if let Some(mark) = mark {
                        cx.scene.text(mark, Point::new(r.x - mark.size().w - 6.0, r.y), p.muted);
                    }
                    if *quote {
                        cx.scene.fill(Rect::new(r.x - 14.0, r.y, 3.0, r.h), 1.5, p.line, None);
                    }
                    cx.scene.text(text, Point::new(r.x, r.y), if *quote { p.muted } else { content });
                }
                LaidKind::Code { lines, line } => {
                    cx.scene.paint(r, theme.small_radius(), &theme.paint(Surface::Inset));
                    // Long lines are cut off at the edge, not wrapped: code keeps its shape.
                    cx.scene.push_clip(r.inset(4.0));
                    for (i, text) in lines.iter().enumerate() {
                        cx.scene.text(text, Point::new(r.x + CODE_PAD, r.y + CODE_PAD + i as f32 * line), p.text);
                    }
                    cx.scene.pop_clip();
                }
                LaidKind::Rule => cx.scene.fill(r, 0.0, p.line, None),
            }
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        match event {
            Event::PointerMoved { pos } if b.contains(*pos) && self.on_link.is_some() => {
                if self.link_at(b.origin(), *pos).is_some() {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => match (self.link_at(b.origin(), *pos), &self.on_link) {
                (Some(to), Some(f)) => {
                    cx.emit(f(to.to_owned()));
                    Status::Captured
                }
                _ => Status::Ignored,
            },
            _ => Status::Ignored,
        }
    }
}

impl<M: 'static> From<Markdown<M>> for Element<M> {
    fn from(w: Markdown<M>) -> Self {
        Element::new(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Vec<(String, &'static str)> {
        inlines(text).into_iter().map(|i| (i.text, match (i.bold, i.italic, i.code, i.link.is_some()) { (_, _, _, true) => "link", (_, _, true, _) => "code", (true, true, ..) => "both", (true, ..) => "bold", (_, true, ..) => "italic", _ => "plain" })).collect()
    }

    fn of(parts: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
        parts.iter().map(|(t, k)| ((*t).to_owned(), *k)).collect()
    }

    #[test]
    fn the_marks_in_a_line_of_prose_are_read() {
        assert_eq!(said("plain words"), of(&[("plain words", "plain")]));
        assert_eq!(said("a **bold** and *slanted* word, and __both *ways*__"), of(&[("a ", "plain"), ("bold", "bold"), (" and ", "plain"), ("slanted", "italic"), (" word, and ", "plain"), ("both ", "bold"), ("ways", "both")]));
        assert_eq!(said("run `cargo test --all` now"), of(&[("run ", "plain"), ("cargo test --all", "code"), (" now", "plain")]));
        assert_eq!(said("stars in `a * b * c` are code"), of(&[("stars in ", "plain"), ("a * b * c", "code"), (" are code", "plain")]));
        let link = inlines("see [the docs](https://example.com/a_b) or https://example.org/x.");
        assert_eq!(link.iter().map(|i| (i.text.as_str(), i.link.as_deref())).collect::<Vec<_>>(), [("see ", None), ("the docs", Some("https://example.com/a_b")), (" or ", None), ("https://example.org/x", Some("https://example.org/x")), (".", None)]);
        // What is only itself: a name with underscores, a sum, a mark with a backslash before it.
        assert_eq!(said("snake_case_name and 2 * 3 * 4 and \\*not\\*"), of(&[("snake_case_name and 2 * 3 * 4 and *not*", "plain")]));
        // Not finished yet, as while it is still arriving: shown as it stands.
        assert_eq!(said("a **bold start"), of(&[("a **bold start", "plain")]));
        assert_eq!(said("some `code not closed"), of(&[("some `code not closed", "plain")]));
        assert_eq!(said("a [link](not done"), of(&[("a [link](not done", "plain")]));
        assert_eq!(said(""), of(&[]));
    }

    #[test]
    fn a_text_is_read_into_its_pieces() {
        let text = "# Plan\n\nFirst a paragraph\nover two lines.\n\n- one\n- two\n  - under two\n1. first\n2) second\n\n> quoted\n> again\n\n---\n\n```rust\nfn main() {}\n\n// done\n```\nAfter.";
        let pieces = blocks(text);
        let shape: Vec<String> = pieces
            .iter()
            .map(|b| match b {
                Block::Prose { kind, mark, depth, inlines } => format!("{kind:?} {}{} {:?}", mark.clone().unwrap_or_default(), depth, inlines.iter().map(|i| i.text.as_str()).collect::<String>()),
                Block::Code { language, text } => format!("code {language} {text:?}"),
                Block::Rule => "rule".to_owned(),
            })
            .collect();
        assert_eq!(
            shape,
            ["Heading(1) 0 \"Plan\"", "Paragraph 0 \"First a paragraph over two lines.\"", "Paragraph •0 \"one\"", "Paragraph •0 \"two\"", "Paragraph •1 \"under two\"", "Paragraph 1.0 \"first\"", "Paragraph 2.0 \"second\"", "Quote 0 \"quoted again\"", "rule", "code rust \"fn main() {}\\n\\n// done\"", "Paragraph 0 \"After.\""]
        );
        // A fence still open, as while the code is arriving: code to the end.
        assert_eq!(blocks("Here:\n```sh\nls -la\ncd src"), [Block::Prose { kind: Kind::Paragraph, mark: None, depth: 0, inlines: inlines("Here:") }, Block::Code { language: "sh".into(), text: "ls -la\ncd src".into() }]);
        assert_eq!(blocks("####### not a heading\n#also not"), [Block::Prose { kind: Kind::Paragraph, mark: None, depth: 0, inlines: inlines("####### not a heading #also not") }]);
        assert!(blocks("").is_empty() && blocks("\n\n  \n").is_empty());
    }
}
