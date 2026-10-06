//! Maps terminal colours to Neo's palette.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};
use neo::theme::Palette;
use neo::{Color, Scheme};

/// The 16 ANSI colours plus foreground and background for one scheme.
#[derive(Clone, Copy, Debug)]
pub struct TermColors {
    pub ansi: [Color; 16],
    pub fg: Color,
    pub bg: Color,
    pub cursor: Color,
    pub on_cursor: Color,
    pub selection: Color,
}

impl TermColors {
    pub fn new(p: &Palette, scheme: Scheme) -> Self {
        let hex = Color::hex;
        let ansi = match scheme {
            // Monokai Pro, with Neo's royal blue in place of Monokai's orange "blue".
            Scheme::Dark => [
                hex(0x403E41), hex(0xFF6188), hex(0xA9DC76), hex(0xFFD866), hex(0x889FEC), hex(0xAB9DF2), hex(0x78DCE8), hex(0xD8D8D6),
                hex(0x727072), hex(0xFF7A9C), hex(0xBDE88F), hex(0xFFE08A), hex(0xA5B7F2), hex(0xC1B6F6), hex(0x9AE6EF), hex(0xFCFCFA),
            ],
            // Darker tones that keep 4.5:1 on the light background.
            Scheme::Light => [
                hex(0x2B3240), hex(0xB5302A), hex(0x1E7A4F), hex(0x8A5D08), hex(0x3F5BC4), hex(0x8A3FB5), hex(0x16706A), hex(0x6B7383),
                hex(0x5C6576), hex(0xC0443A), hex(0x1E8A5A), hex(0x9A6A0E), hex(0x4A67D6), hex(0x9B51C6), hex(0x1C7F72), hex(0x2B3240),
            ],
        };
        Self { ansi, fg: p.text, bg: p.bg, cursor: p.accent, on_cursor: p.on_accent, selection: p.accent.with_alpha(0.35) }
    }

    fn rgb(c: Rgb) -> Color {
        Color::rgb(c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0)
    }

    /// Resolves a cell colour, honouring colours the program changed with OSC 4.
    pub fn resolve(&self, c: AnsiColor, overrides: &Colors) -> Color {
        match c {
            AnsiColor::Spec(rgb) => Self::rgb(rgb),
            AnsiColor::Indexed(i) => self.indexed(i as usize, overrides),
            AnsiColor::Named(n) => self.named(n, overrides),
        }
    }

    fn indexed(&self, i: usize, overrides: &Colors) -> Color {
        if let Some(c) = overrides[i] {
            return Self::rgb(c);
        }
        match i {
            0..16 => self.ansi[i],
            16..232 => {
                let i = i - 16;
                let level = |v: usize| if v == 0 { 0.0 } else { (55 + v * 40) as f32 / 255.0 };
                Color::rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
            }
            _ => {
                let v = (8 + (i - 232) * 10) as f32 / 255.0;
                Color::rgb(v, v, v)
            }
        }
    }

    fn named(&self, n: NamedColor, overrides: &Colors) -> Color {
        let i = n as usize;
        if let Some(c) = overrides[i] {
            return Self::rgb(c);
        }
        match n {
            NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
            NamedColor::Background => self.bg,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => self.fg.mix(self.bg, 0.35),
            _ if i < 16 => self.ansi[i],
            // Dim black through dim white follow the foreground colours.
            _ => self.ansi[(i - NamedColor::DimBlack as usize).min(7)].mix(self.bg, 0.35),
        }
    }

    /// The bright variant of ANSI colours 0 to 7, for bold text.
    pub fn brighten(&self, c: AnsiColor) -> AnsiColor {
        match c {
            AnsiColor::Named(n) if (n as usize) < 8 => AnsiColor::Indexed(n as u8 + 8),
            AnsiColor::Indexed(i) if i < 8 => AnsiColor::Indexed(i + 8),
            c => c,
        }
    }
}
