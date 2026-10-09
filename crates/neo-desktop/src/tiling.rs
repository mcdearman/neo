//! How windows are laid out: left where they are put, or tiled.
//!
//! Tiled, the windows on a screen share it between them with none over
//! another, and are laid out afresh as windows come and go, the way a
//! dynamic tiling window manager does it. This is the part that knows
//! the layouts and keeps the settings; NeoShell is the part that moves
//! the windows.

use std::path::PathBuf;

use neo::Rect;

/// Whether windows are tiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Windows stay where they are put, as on any desktop.
    Floating,
    /// Windows are laid out to share the screen.
    Tiling,
}

/// How tiled windows share a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One window, or a few, large on the left; the rest stacked on the right.
    MasterStack,
    /// Side by side, each the screen's height.
    Columns,
    /// In rows and columns, as near square as their number allows.
    Grid,
    /// Each fills the screen; one is seen at a time.
    Monocle,
}

impl Layout {
    pub const ALL: [Layout; 4] = [Layout::MasterStack, Layout::Columns, Layout::Grid, Layout::Monocle];

    pub fn id(self) -> &'static str {
        match self {
            Layout::MasterStack => "master-stack",
            Layout::Columns => "columns",
            Layout::Grid => "grid",
            Layout::Monocle => "monocle",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::MasterStack => "Master and stack",
            Layout::Columns => "Columns",
            Layout::Grid => "Grid",
            Layout::Monocle => "Monocle",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Layout::MasterStack => "The main window is large on the left, and the rest are stacked on the right.",
            Layout::Columns => "The windows stand side by side, each the height of the screen.",
            Layout::Grid => "The windows are in rows and columns, as near square as their number allows.",
            Layout::Monocle => "Every window fills the screen, and one is seen at a time.",
        }
    }

    /// The one after this, round again to the first.
    pub fn next(self) -> Self {
        Self::ALL[(Self::ALL.iter().position(|l| *l == self).unwrap_or(0) + 1) % Self::ALL.len()]
    }
}

/// Where a window that has just opened goes among the tiled ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewWindow {
    /// It becomes the main window, and the one that was moves to the stack.
    Master,
    /// It goes to the end of the stack.
    Stack,
}

/// The windowing settings, kept as `key = value` lines in `windowing.conf`.
#[derive(Clone, Debug, PartialEq)]
pub struct Windowing {
    pub mode: Mode,
    pub layout: Layout,
    /// The share of the screen's width the main windows take, in the
    /// master and stack layout.
    pub ratio: f32,
    /// How many windows are main ones.
    pub masters: u32,
    /// The room between one window and the next, in points.
    pub gap: f32,
    /// The room between the windows and the screen's edges.
    pub margin: f32,
    pub new_window: NewWindow,
    /// Apps whose windows are never tiled, by name.
    pub floating: Vec<String>,
}

/// The least and most of the screen the main windows may take.
pub const RATIOS: (f32, f32) = (0.3, 0.8);
/// The most windows that can be main ones.
pub const MOST_MASTERS: u32 = 4;
/// The widest the gaps can be.
pub const WIDEST_GAP: f32 = 40.0;

impl Default for Windowing {
    fn default() -> Self {
        Self { mode: Mode::Floating, layout: Layout::MasterStack, ratio: 0.55, masters: 1, gap: 8.0, margin: 8.0, new_window: NewWindow::Stack, floating: vec![] }
    }
}

impl Windowing {
    pub fn path() -> PathBuf {
        crate::config_dir().join("windowing.conf")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).map(|text| Self::parse(&text)).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.encode())
    }

    pub fn encode(&self) -> String {
        let mut out = format!("mode = {}\nlayout = {}\nratio = {:.2}\nmasters = {}\ngap = {:.0}\nmargin = {:.0}\nnew-window = {}\n", if self.mode == Mode::Tiling { "tiling" } else { "floating" }, self.layout.id(), self.ratio, self.masters, self.gap, self.margin, if self.new_window == NewWindow::Master { "master" } else { "stack" });
        for app in &self.floating {
            out.push_str(&format!("float = {app}\n"));
        }
        out
    }

    /// Reads what [`encode`](Self::encode) wrote. What is missing or makes
    /// no sense keeps its default, and numbers are kept within bounds.
    pub fn parse(text: &str) -> Self {
        let mut w = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "mode" => w.mode = if value == "tiling" { Mode::Tiling } else { Mode::Floating },
                "layout" => w.layout = Layout::ALL.into_iter().find(|l| l.id() == value).unwrap_or(w.layout),
                "ratio" => w.ratio = value.parse().map_or(w.ratio, |r: f32| r.clamp(RATIOS.0, RATIOS.1)),
                "masters" => w.masters = value.parse().map_or(w.masters, |m: u32| m.clamp(1, MOST_MASTERS)),
                "gap" => w.gap = value.parse().map_or(w.gap, |g: f32| g.clamp(0.0, WIDEST_GAP)),
                "margin" => w.margin = value.parse().map_or(w.margin, |g: f32| g.clamp(0.0, WIDEST_GAP)),
                "new-window" => w.new_window = if value == "master" { NewWindow::Master } else { NewWindow::Stack },
                "float" if !value.is_empty() && !w.floating.iter().any(|a| a == value) => w.floating.push(value.to_owned()),
                _ => {}
            }
        }
        w
    }

    /// Whether an app's windows are left out of the tiling.
    pub fn floats(&self, app: &str) -> bool {
        self.floating.iter().any(|a| a.eq_ignore_ascii_case(app))
    }

    /// Where each of `n` windows goes in `area`, the first being the main
    /// one: the layout, with the margin kept round the edge and the gap
    /// between one window and the next.
    pub fn arrange(&self, n: usize, area: Rect) -> Vec<Rect> {
        if n == 0 {
            return vec![];
        }
        let inside = Rect::new(area.x + self.margin, area.y + self.margin, (area.w - self.margin * 2.0).max(1.0), (area.h - self.margin * 2.0).max(1.0));
        let whole = |r: Rect| Rect::new(r.x.round(), r.y.round(), r.w.round().max(1.0), r.h.round().max(1.0));
        // `count` rectangles down a column, or across a row, with the gap between.
        let down = |column: Rect, count: usize| -> Vec<Rect> {
            let each = (column.h - self.gap * (count as f32 - 1.0)) / count as f32;
            (0..count).map(|i| whole(Rect::new(column.x, column.y + i as f32 * (each + self.gap), column.w, each))).collect()
        };
        let across = |row: Rect, count: usize| -> Vec<Rect> {
            let each = (row.w - self.gap * (count as f32 - 1.0)) / count as f32;
            (0..count).map(|i| whole(Rect::new(row.x + i as f32 * (each + self.gap), row.y, each, row.h))).collect()
        };
        match self.layout {
            Layout::Monocle => vec![whole(inside); n],
            Layout::Columns => across(inside, n),
            Layout::Grid => {
                let columns = (n as f32).sqrt().ceil() as usize;
                let rows = n.div_ceil(columns);
                let mut out = Vec::with_capacity(n);
                for (r, strip) in down(inside, rows).into_iter().enumerate() {
                    // The last row has what is left, and they share its width.
                    let in_row = if r + 1 == rows { n - columns * (rows - 1) } else { columns };
                    out.extend(across(strip, in_row));
                }
                out
            }
            Layout::MasterStack => {
                let masters = (self.masters as usize).clamp(1, n);
                // With nothing to stack, the main windows have the screen.
                if masters == n {
                    return down(inside, n);
                }
                let wide = ((inside.w - self.gap) * self.ratio).round();
                let mut out = down(Rect::new(inside.x, inside.y, wide, inside.h), masters);
                out.extend(down(Rect::new(inside.x + wide + self.gap, inside.y, inside.w - wide - self.gap, inside.h), n - masters));
                out
            }
        }
    }
}

/// The keys that work the tiling while it is on, each held with Control
/// and Option (Alt): the key, and what it does. NeoShell binds them and
/// Settings lists them.
pub const KEYS: [(&str, &str); 10] =
    [("J", "Go to the next window"), ("K", "Go to the window before"), ("Return", "Make this the main window"), ("L", "Give the main window more room"), ("H", "Give the main window less room"), (".", "One more main window"), (",", "One fewer main window"), ("Space", "Change to the next layout"), ("F", "Let this window float, or tile it again"), ("R", "Lay the windows out again")];

/// What NeoShell says of how the tiling is going, for Settings to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Whether it has been given leave to move other apps' windows.
    pub allowed: bool,
    /// How many windows it is laying out.
    pub windows: u32,
    /// Whether this system's windows can be laid out by it at all.
    pub possible: bool,
}

impl Status {
    pub fn path() -> PathBuf {
        crate::config_dir().join("windowing.status")
    }

    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()).ok()?;
        let said = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)).map(str::trim);
        Some(Self { allowed: said("allowed =") == Some("true"), windows: said("windows =").and_then(|v| v.parse().ok()).unwrap_or(0), possible: said("possible =") != Some("false") })
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, format!("allowed = {}\nwindows = {}\npossible = {}\n", self.allowed, self.windows, self.possible))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { x: 0.0, y: 30.0, w: 1440.0, h: 870.0 };

    fn plain() -> Windowing {
        Windowing { mode: Mode::Tiling, gap: 10.0, margin: 20.0, ..Windowing::default() }
    }

    fn check(rects: &[Rect], w: &Windowing) {
        for (i, a) in rects.iter().enumerate() {
            assert!(a.x >= SCREEN.x + w.margin - 1.0 && a.y >= SCREEN.y + w.margin - 1.0 && a.right() <= SCREEN.right() - w.margin + 1.0 && a.bottom() <= SCREEN.bottom() - w.margin + 1.0, "inside the margin: {a:?}");
            if w.layout != Layout::Monocle {
                for b in &rects[i + 1..] {
                    assert!(a.right() <= b.x + 0.5 || b.right() <= a.x + 0.5 || a.bottom() <= b.y + 0.5 || b.bottom() <= a.y + 0.5, "{a:?} and {b:?} overlap");
                }
            }
        }
    }

    #[test]
    fn a_main_window_and_a_stack() {
        let w = plain();
        assert_eq!(w.arrange(0, SCREEN), vec![]);
        assert_eq!(w.arrange(1, SCREEN), [Rect::new(20.0, 50.0, 1400.0, 830.0)], "one window has the screen, less the margin");
        let two = w.arrange(2, SCREEN);
        assert_eq!(two, [Rect::new(20.0, 50.0, 765.0, 830.0), Rect::new(795.0, 50.0, 625.0, 830.0)], "55% for the main one, the gap, and the rest");
        let four = w.arrange(4, SCREEN);
        assert_eq!((four[0], four[1].x, four[1].w), (two[0], 795.0, 625.0), "the main window keeps its place as the stack fills");
        assert_eq!((four[1].y, four[2].y - four[1].bottom(), four[3].bottom()), (50.0, 10.0, 880.0), "the stack shares the height, with the gap between");
        check(&four, &w);
        // More main windows, and a wider share for them.
        let w = Windowing { masters: 2, ratio: 0.7, ..plain() };
        let five = w.arrange(5, SCREEN);
        assert_eq!((five[0].x, five[1].x, five[2].x, five[0].w), (20.0, 20.0, five[4].x, 973.0), "two on the left, three on the right");
        assert_eq!(w.arrange(2, SCREEN).iter().map(|r| r.w).collect::<Vec<_>>(), [1400.0, 1400.0], "with none to stack, the main ones have it all");
        check(&five, &w);
    }

    #[test]
    fn columns_a_grid_and_one_at_a_time() {
        let w = Windowing { layout: Layout::Columns, ..plain() };
        let three = w.arrange(3, SCREEN);
        assert_eq!((three.len(), three[0].h, three[0].w, three[1].x - three[0].right()), (3, 830.0, 460.0, 10.0));
        check(&three, &w);
        let w = Windowing { layout: Layout::Grid, ..plain() };
        let five = w.arrange(5, SCREEN);
        assert_eq!((five[0].y, five[2].y, five[3].y, five[3].w, five[4].right()), (50.0, 50.0, five[4].y, 695.0, 1420.0), "three above and two below, the two sharing the width");
        assert_eq!(w.arrange(4, SCREEN).iter().map(|r| (r.w, r.h)).collect::<Vec<_>>(), vec![(695.0, 410.0); 4], "four are two by two");
        check(&five, &w);
        let w = Windowing { layout: Layout::Monocle, ..plain() };
        assert_eq!(w.arrange(3, SCREEN), vec![Rect::new(20.0, 50.0, 1400.0, 830.0); 3]);
        for n in 1..=9 {
            for layout in Layout::ALL {
                let w = Windowing { layout, masters: 2, ..plain() };
                let rects = w.arrange(n, SCREEN);
                assert_eq!(rects.len(), n);
                check(&rects, &w);
            }
        }
    }

    #[test]
    fn the_settings_are_written_and_read_back() {
        let w = Windowing { mode: Mode::Tiling, layout: Layout::Grid, ratio: 0.65, masters: 2, gap: 12.0, margin: 4.0, new_window: NewWindow::Master, floating: vec!["System Settings".into(), "NeoCal".into()] };
        assert_eq!(Windowing::parse(&w.encode()), w);
        assert_eq!(Windowing::parse(""), Windowing::default());
        assert_eq!(Windowing::default().mode, Mode::Floating, "nothing is moved until it is asked for");
        // What makes no sense is kept within bounds or left as it was.
        let odd = Windowing::parse("mode = sideways\nlayout = spiral\nratio = 9\nmasters = 0\ngap = -4\nmargin = 900\nfloat = \nfloat = Notes\nfloat = Notes\n");
        assert_eq!((odd.mode, odd.layout, odd.ratio, odd.masters, odd.gap, odd.margin, odd.floating.clone()), (Mode::Floating, Layout::MasterStack, 0.8, 1, 0.0, 40.0, vec!["Notes".to_owned()]));
        assert!(odd.floats("notes") && !odd.floats("Safari"));
        assert_eq!(Layout::ALL.map(Layout::next), [Layout::Columns, Layout::Grid, Layout::Monocle, Layout::MasterStack]);
    }
}
