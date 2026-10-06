//! A treemap: a folder drawn as a rectangle cut into smaller ones, each
//! with an area in proportion to its size, so what is big is plain to see.

use std::rc::Rc;

use neo::prelude::*;
use neo::{CursorIcon, Cx, DrawCx, Event, EventCx, Limits, Point, PointerButton, Rect, Size, Status, TextLayout, TextStyle, ThemeCx, Widget};

use crate::scan::Entry;

/// Cuts `area` into one rectangle per size, in order, with areas in
/// proportion and shapes kept as near square as the sizes allow (the
/// "squarified" layout of Bruls, Huizing and van Wijk). Give the sizes
/// largest first for the best shapes.
pub fn squarify(sizes: &[f64], area: Rect) -> Vec<Rect> {
    let total: f64 = sizes.iter().sum();
    if total <= 0.0 || area.w <= 0.0 || area.h <= 0.0 {
        return vec![Rect::ZERO; sizes.len()];
    }
    let scale = f64::from(area.w) * f64::from(area.h) / total;
    let areas: Vec<f64> = sizes.iter().map(|s| s.max(0.0) * scale).collect();
    let (mut x, mut y, mut w, mut h) = (f64::from(area.x), f64::from(area.y), f64::from(area.w), f64::from(area.h));
    let mut out = Vec::with_capacity(sizes.len());
    let mut i = 0;
    while i < areas.len() {
        // Take as many as keep the row's worst shape from getting worse.
        let side = w.min(h).max(f64::MIN_POSITIVE);
        let (mut sum, mut least, mut most, mut best) = (0.0f64, f64::INFINITY, 0.0f64, f64::INFINITY);
        let mut j = i;
        while j < areas.len() {
            let (s, lo, hi) = (sum + areas[j], least.min(areas[j]), most.max(areas[j]));
            let worst = if s > 0.0 && lo > 0.0 { (side * side * hi / (s * s)).max(s * s / (side * side * lo)) } else { f64::INFINITY };
            if j > i && worst > best {
                break;
            }
            (sum, least, most, best) = (s, lo, hi, worst);
            j += 1;
        }
        // The row goes along the shorter side, as thick as its total needs.
        let thick = sum / side;
        let wide = w >= h;
        let mut along = 0.0;
        for a in &areas[i..j] {
            let len = if thick > 0.0 { a / thick } else { 0.0 };
            out.push(if wide { Rect::new(x as f32, (y + along) as f32, thick as f32, len as f32) } else { Rect::new((x + along) as f32, y as f32, len as f32, thick as f32) });
            along += len;
        }
        if wide {
            x += thick;
            w -= thick;
        } else {
            y += thick;
            h -= thick;
        }
        i = j;
    }
    out
}

/// One thing in the map, with what is inside it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub size: u64,
    pub dir: bool,
    pub children: Vec<Item>,
}

/// How many levels are drawn inside each other, and how many things per
/// folder at each: beyond these the rectangles are too small to see.
const MOST: [usize; 3] = [160, 48, 16];

/// The contents of a folder, as far down as the map draws.
pub fn items(entry: &Entry) -> Vec<Item> {
    fn level(entry: &Entry, depth: usize) -> Vec<Item> {
        let Some(most) = MOST.get(depth) else {
            return vec![];
        };
        entry.children.iter().take(*most).filter(|c| c.size > 0).map(|c| Item { name: c.name.clone(), size: c.size, dir: c.dir, children: level(c, depth + 1) }).collect()
    }
    level(entry, 0)
}

/// Colours for the things at the top level; what is inside each takes a
/// shade of its colour, so a folder reads as one family.
const COLOURS: [u32; 10] = [0x4C6FD9, 0x2F9E8F, 0xC9643C, 0x8A63D2, 0xB8862B, 0x3D8FBF, 0xC2527F, 0x5E9A45, 0x7A7F8C, 0xA65FA8];

struct Tile {
    /// Where it is, relative to the widget.
    rect: Rect,
    /// The indices that lead to its item, and how many of them count.
    path: [usize; 3],
    depth: usize,
    colour: Color,
    /// A folder with its contents drawn inside, which shows as a frame.
    frame: bool,
    label: Option<TextLayout>,
}

#[derive(Default)]
struct MapState {
    hover: Option<usize>,
}

/// Height of the line under the map that names what the pointer is on.
const STRIP: f32 = 26.0;
const HEADER: f32 = 18.0;

/// A folder's contents as a treemap. Hovering names what is under the
/// pointer; clicking opens the top-level thing it belongs to.
pub struct Treemap<M> {
    items: Rc<Vec<Item>>,
    /// The size of the folder shown, for percentages.
    total: u64,
    on_open: Option<Box<dyn Fn(usize) -> M>>,
    tiles: Vec<Tile>,
}

impl<M> Treemap<M> {
    pub fn new(items: Rc<Vec<Item>>, total: u64) -> Self {
        Self { items, total, on_open: None, tiles: vec![] }
    }

    /// Called with the index of a top-level thing when it is clicked.
    pub fn on_open(mut self, f: impl Fn(usize) -> M + 'static) -> Self {
        self.on_open = Some(Box::new(f));
        self
    }

    fn item(&self, t: &Tile) -> Option<&Item> {
        let mut item = self.items.get(t.path[0])?;
        for i in &t.path[1..=t.depth] {
            item = item.children.get(*i)?;
        }
        Some(item)
    }

    /// The names from the top down to a tile, as a path.
    fn name(&self, t: &Tile) -> String {
        let mut parts = vec![];
        let mut list = &*self.items;
        for i in &t.path[..=t.depth] {
            let Some(item) = list.get(*i) else { break };
            parts.push(item.name.as_str());
            list = &item.children;
        }
        parts.join("/")
    }

    fn place(&mut self, cx: &mut Cx, style: &TextStyle, area: Rect, path: [usize; 3], depth: usize, base: Option<Color>) {
        let list: Vec<(String, u64, bool, bool)> = {
            let mut list = &*self.items;
            for i in &path[..depth] {
                list = &list[*i].children;
            }
            list.iter().map(|i| (i.name.clone(), i.size, i.dir, !i.children.is_empty())).collect()
        };
        let rects = squarify(&list.iter().map(|i| i.1 as f64).collect::<Vec<_>>(), area);
        for (i, ((name, size, dir, has_children), rect)) in list.into_iter().zip(rects).enumerate() {
            // Too small to see or point at.
            if rect.w < 2.0 || rect.h < 2.0 {
                continue;
            }
            let mut path = path;
            path[depth] = i;
            let colour = match base {
                None => Color::hex(COLOURS[i % COLOURS.len()]),
                // Shades of the family's colour, so neighbours can be told apart.
                Some(b) => b.mix(if i % 2 == 0 { Color::WHITE } else { Color::BLACK }, 0.07 + 0.05 * (i % 3) as f32),
            };
            let inside = rect.inset(2.0);
            let frame = dir && has_children && depth + 1 < MOST.len() && inside.w > 22.0 && inside.h > 22.0;
            let titled = frame && depth == 0 && rect.w > 70.0 && rect.h > 46.0;
            let room = rect.w > 64.0 && rect.h > 30.0;
            let label = if titled {
                Some(cx.text().layout(&name, style, None))
            } else if !frame && room {
                Some(cx.text().layout(&format!("{name}\n{}", neo_desktop::fs::human_size(size)), style, None))
            } else {
                None
            };
            self.tiles.push(Tile { rect, path, depth, colour, frame, label });
            if frame {
                let inner = if titled { Rect::new(inside.x, inside.y + HEADER, inside.w, inside.h - HEADER) } else { inside };
                self.place(cx, style, inner, path, depth + 1, Some(base.unwrap_or(colour)));
            }
        }
    }

    fn hit(&self, origin: Point, p: Point) -> Option<usize> {
        // The last one drawn is the innermost.
        self.tiles.iter().rposition(|t| Rect::new(origin.x + t.rect.x, origin.y + t.rect.y, t.rect.w, t.rect.h).contains(p))
    }
}

impl<M: 'static> Widget<M> for Treemap<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = limits.max;
        let mut style = cx.theme().text(TextRole::Caption).style();
        style.line_height = 1.25;
        self.tiles.clear();
        self.place(cx, &style, Rect::new(0.0, 0.0, size.w, (size.h - STRIP).max(0.0)), [0; 3], 0, None);
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let hover = cx.state::<MapState>().hover.filter(|i| *i < self.tiles.len());
        cx.scene.push_clip(b);
        for t in &self.tiles {
            let r = Rect::new(b.x + t.rect.x, b.y + t.rect.y, t.rect.w, t.rect.h);
            // A folder is a darker frame in its colour, with its contents on it.
            let fill = if t.frame { t.colour.mix(Color::BLACK, 0.45) } else { t.colour };
            cx.scene.fill(r.inset(0.5), 2.0, fill, None);
        }
        // Text after every rectangle, each kept inside its own.
        for t in &self.tiles {
            let Some(label) = &t.label else { continue };
            let r = Rect::new(b.x + t.rect.x, b.y + t.rect.y, t.rect.w, t.rect.h);
            cx.scene.push_clip(r.inset(3.0));
            cx.scene.text(label, Point::new(r.x + 6.0, r.y + if t.frame { 2.0 } else { 5.0 }), Color::WHITE);
            cx.scene.pop_clip();
        }
        if let Some(t) = hover.map(|i| &self.tiles[i]) {
            let r = Rect::new(b.x + t.rect.x, b.y + t.rect.y, t.rect.w, t.rect.h);
            cx.scene.fill(r.inset(0.5), 2.0, Color::WHITE.with_alpha(0.12), Some((2.0, Color::WHITE)));
        }
        cx.scene.pop_clip();
        // The line underneath says what the pointer is on.
        let style = theme.text(TextRole::Caption).style();
        let words = match hover.map(|i| &self.tiles[i]).and_then(|t| self.item(t).map(|item| (t, item))) {
            Some((t, item)) => {
                let share = if self.total > 0 { item.size as f64 / self.total as f64 * 100.0 } else { 0.0 };
                format!("{}  ·  {}  ·  {share:.1}% of this folder", self.name(t), neo_desktop::fs::human_size(item.size))
            }
            None if self.tiles.is_empty() => "Nothing here takes up any space.".to_owned(),
            None => "Each block's area is its size. Point at one to see what it is; click to open it.".to_owned(),
        };
        let line = cx.text().layout(&words, &style, None);
        cx.scene.push_clip(Rect::new(b.x, b.bottom() - STRIP, b.w, STRIP));
        cx.scene.text(&line, Point::new(b.x + 4.0, b.bottom() - STRIP + ((STRIP - line.size().h) * 0.5).round()), if hover.is_some() { p.text } else { p.muted });
        cx.scene.pop_clip();
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let origin = Point::new(b.x, b.y);
        match event {
            Event::PointerMoved { pos } => {
                let hit = self.hit(origin, *pos);
                if hit.is_some() {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                let st = cx.state::<MapState>();
                if st.hover != hit {
                    st.hover = hit;
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerLeft => {
                cx.state::<MapState>().hover = None;
                cx.request_redraw();
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } => match (self.hit(origin, *pos), &self.on_open) {
                (Some(i), Some(open)) => {
                    cx.emit(open(self.tiles[i].path[0]));
                    Status::Captured
                }
                _ => Status::Ignored,
            },
            _ => Status::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(r: &Rect) -> f64 {
        f64::from(r.w) * f64::from(r.h)
    }

    #[test]
    fn areas_are_in_proportion_and_fill_the_space() {
        let sizes = [50.0, 30.0, 10.0, 6.0, 3.0, 1.0];
        let space = Rect::new(10.0, 20.0, 400.0, 300.0);
        let rects = squarify(&sizes, space);
        assert_eq!(rects.len(), sizes.len());
        let total: f64 = rects.iter().map(area).sum();
        assert!((total - 120_000.0).abs() < 1.0, "the rectangles cover the space: {total}");
        for (s, r) in sizes.iter().zip(&rects) {
            assert!((area(r) - s * 1200.0).abs() < 1.0, "{s} gets its share, got {}", area(r));
            assert!(r.x >= 9.99 && r.y >= 19.99 && r.right() <= 410.01 && r.bottom() <= 320.01, "inside the space: {r:?}");
        }
        // None overlap another.
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let overlap = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0) * (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
                assert!(overlap < 0.5, "{a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn shapes_stay_near_square() {
        // Equal sizes in a square: a grid, not sixteen slivers.
        let rects = squarify(&[1.0; 16], Rect::new(0.0, 0.0, 400.0, 400.0));
        for r in &rects {
            let ratio = (r.w / r.h).max(r.h / r.w);
            assert!(ratio < 1.5, "{r:?} is {ratio} to 1");
        }
        // One big thing and many small: the big one is not a thin strip.
        let mut sizes = vec![60.0];
        sizes.extend([1.0; 40]);
        let big = squarify(&sizes, Rect::new(0.0, 0.0, 500.0, 300.0))[0];
        assert!((big.w / big.h).max(big.h / big.w) < 2.5, "{big:?}");
    }

    #[test]
    fn odd_inputs_do_not_break_it() {
        assert!(squarify(&[], Rect::new(0.0, 0.0, 10.0, 10.0)).is_empty());
        assert_eq!(squarify(&[0.0, 0.0], Rect::new(0.0, 0.0, 10.0, 10.0)), [Rect::ZERO; 2]);
        assert_eq!(squarify(&[5.0], Rect::new(0.0, 0.0, 0.0, 10.0)), [Rect::ZERO]);
        let one = squarify(&[5.0], Rect::new(1.0, 2.0, 30.0, 40.0));
        assert_eq!(one, [Rect::new(1.0, 2.0, 30.0, 40.0)], "one thing takes it all");
        // Something of no size among others gets no room, and no NaN.
        let rects = squarify(&[4.0, 0.0, 2.0], Rect::new(0.0, 0.0, 60.0, 10.0));
        assert!(rects.iter().all(|r| r.w.is_finite() && r.h.is_finite()));
        assert!(area(&rects[1]) < 0.01);
    }

    #[test]
    fn the_map_takes_only_what_it_can_draw() {
        let leaf = |name: &str, size: u64| Entry { name: name.into(), size, files: 1, ..Default::default() };
        let folder = |name: &str, children: Vec<Entry>| Entry { name: name.into(), size: children.iter().map(|c| c.size).sum(), dir: true, files: children.len() as u64, children, ..Default::default() };
        let deep = folder("a", vec![folder("b", vec![folder("c", vec![folder("d", vec![leaf("e", 10)])])])]);
        let root = folder("root", vec![deep, leaf("empty", 0), leaf("f", 5)]);
        let map = items(&root);
        assert_eq!(map.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["a", "f"], "nothing of no size");
        let c = &map[0].children[0].children[0];
        assert_eq!((c.name.as_str(), c.children.len()), ("c", 0), "three levels down and no further");
        // A folder with thousands of things shows the largest of them.
        let wide = folder("wide", (0..500).map(|i| leaf(&format!("f{i}"), 1000 - i)).collect());
        assert_eq!(items(&wide).len(), MOST[0]);
    }
}
