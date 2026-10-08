//! A word cloud: the words Apollo's memories are about, the weightiest
//! largest and in the middle, each one a thing to click.

use std::rc::Rc;

use neo::prelude::*;
use neo::{CursorIcon, Cx, DrawCx, Event, EventCx, Limits, Point, PointerButton, Rect, Size, Status, TextLayout, ThemeCx, Widget};
use neo_apollo_core::store::Word;

/// Room kept between one word and the next.
const GAP: f32 = 5.0;
/// How fast the spiral that words are placed along winds outwards, in
/// pixels for each radian, and how far apart the spots tried along it are.
const WIND: f32 = 2.5;
const STEP: f32 = 7.0;
/// The smallest and largest a word is drawn.
const SIZES: (f32, f32) = (13.0, 46.0);

/// The first spot along a spiral out from `centre`, starting `from` away
/// from it, where a rectangle of `size` is inside `area` and clear of
/// everything in `placed`. `turn` is the angle the spiral starts at, and
/// `stretch` how much wider than tall it is.
fn spiral(size: Size, centre: Point, from: f32, turn: f32, stretch: f32, area: Size, placed: &[Rect]) -> Option<Rect> {
    // Past the furthest corner there is nowhere left to look.
    let reach = (area.w * area.w + area.h * area.h).sqrt();
    let mut t = 0.0f32;
    // One too large for the area has no place in it at all.
    if size.w > area.w || size.h > area.h {
        return None;
    }
    loop {
        let r = from + t * WIND;
        if r > reach {
            return None;
        }
        let at = Rect::new((centre.x + r * (t + turn).cos() * stretch - size.w / 2.0).round(), (centre.y + r * (t + turn).sin() - size.h / 2.0).round(), size.w, size.h);
        let inside = at.x >= 0.0 && at.y >= 0.0 && at.right() <= area.w && at.bottom() <= area.h;
        if inside && !placed.iter().any(|p| at.x < p.right() + GAP && p.x < at.right() + GAP && at.y < p.bottom() + GAP && p.y < at.bottom() + GAP) {
            return Some(at);
        }
        // Steps of about the same length along the spiral wherever it has got to.
        t += STEP / r.max(STEP * 2.0);
    }
}

/// Finds each rectangle a place in `area`, in the order given: the first
/// in the middle, and each after it at the first spot along a spiral out
/// from the middle where it touches none already placed. `None` for one
/// that there is no room left for.
pub fn place(sizes: &[Size], area: Size) -> Vec<Option<Rect>> {
    let centre = Point::new(area.w / 2.0, area.h / 2.0);
    // The spiral is stretched to the area's shape, so a wide area is filled wide.
    let stretch = if area.h > 0.0 { (area.w / area.h).clamp(0.4, 3.0) } else { 1.0 };
    let mut placed: Vec<Rect> = vec![];
    let mut out = Vec::with_capacity(sizes.len());
    for (i, size) in sizes.iter().enumerate() {
        // Each starts at its own angle, so they do not queue along one arm.
        let found = spiral(*size, centre, 0.0, i as f32 * 2.4, stretch, area, &placed);
        placed.extend(found);
        out.push(found);
    }
    out
}

/// A word that was picked, and the words that go with it.
#[derive(Clone, Debug, PartialEq)]
pub struct Hub {
    pub word: String,
    /// Each with how strongly it goes, from 0 to 1.
    pub related: Vec<(String, f32)>,
}

/// A word placed in the graph.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub word: String,
    pub rect: Rect,
    /// One of the picked words, as against one that goes with them.
    pub hub: bool,
    pub weight: f32,
}

/// The graph laid out: its words, and which are joined to which.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arranged {
    pub nodes: Vec<Node>,
    /// Pairs of indices into `nodes`, the lower first.
    pub edges: Vec<(usize, usize)>,
}

/// How large a picked word is drawn, and one that goes with it.
const HUB_SIZE: f32 = 28.0;
pub fn satellite_size(weight: f32) -> f32 {
    (13.0 + 8.0 * weight.clamp(0.0, 1.0)).round()
}

/// Lays the picked words out across `area`, in the order they were
/// picked and each joined to the next, with the words that go with each
/// gathered round it and joined to it. A word that goes with more than
/// one of them is there once, joined to each. `measure` says how large a
/// word is drawn: it is given the word, whether it is a picked one, and
/// its weight.
pub fn arrange(hubs: &[Hub], measure: &mut dyn FnMut(&str, bool, f32) -> Size, area: Size) -> Arranged {
    let mut out = Arranged::default();
    if hubs.is_empty() || area.w <= 0.0 || area.h <= 0.0 {
        return out;
    }
    let column = area.w / hubs.len() as f32;
    let centres: Vec<Point> = (0..hubs.len()).map(|i| Point::new(((i as f32 + 0.5) * column).round(), (area.h / 2.0).round())).collect();
    for (hub, centre) in hubs.iter().zip(&centres) {
        let size = measure(&hub.word, true, 1.0);
        out.nodes.push(Node { word: hub.word.clone(), rect: Rect::new((centre.x - size.w / 2.0).round(), (centre.y - size.h / 2.0).round(), size.w, size.h), hub: true, weight: 1.0 });
    }
    let join = |edges: &mut Vec<(usize, usize)>, a: usize, b: usize| {
        let edge = (a.min(b), a.max(b));
        if a != b && !edges.contains(&edge) {
            edges.push(edge);
        }
    };
    for i in 1..hubs.len() {
        join(&mut out.edges, i - 1, i);
    }
    // Round each picked word, a ring that is as wide as its share of the
    // area allows and as tall as the area does.
    let (wide, tall) = ((column * 0.36).clamp(60.0, 230.0), (area.h * 0.36).max(40.0));
    for (i, hub) in hubs.iter().enumerate() {
        let own: Vec<&(String, f32)> = hub.related.iter().filter(|(w, _)| !hubs.iter().any(|h| &h.word == w)).collect();
        let mut nth = 0;
        for (word, weight) in &hub.related {
            if let Some(there) = out.nodes.iter().position(|node| &node.word == word) {
                join(&mut out.edges, i, there);
                continue;
            }
            let size = measure(word, false, *weight);
            let mut taken: Vec<Rect> = out.nodes.iter().map(|node| if node.hub { node.rect.inset(-10.0) } else { node.rect }).collect();
            // And nothing on the line from one picked word to the next.
            taken.extend(centres.windows(2).map(|c| Rect::new(c[0].x, c[0].y - 5.0, c[1].x - c[0].x, 10.0)));
            // One that goes with another picked word as well sits between the two.
            let other = hubs.iter().position(|h| h.word != hub.word && h.related.iter().any(|(w, _)| w == word));
            let rect = match other {
                Some(j) => {
                    let between = Point::new((centres[i].x + centres[j].x) / 2.0, centres[i].y);
                    spiral(size, between, 34.0, std::f32::consts::FRAC_PI_2 + nth as f32 * std::f32::consts::PI, 1.0, area, &taken)
                }
                // The rest evenly round it, every other one a little nearer
                // in, so that neighbours have room.
                None => {
                    let angle = nth as f32 / own.len().max(1) as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    let near = if nth % 2 == 1 && own.len() > 6 { 0.72 } else { 1.0 };
                    spiral(size, centres[i], tall * near, angle, wide / tall, area, &taken)
                }
            };
            nth += 1;
            if let Some(rect) = rect {
                out.nodes.push(Node { word: word.clone(), rect, hub: false, weight: *weight });
                join(&mut out.edges, i, out.nodes.len() - 1);
            }
        }
    }
    out
}

/// The line that joins two words: between their middles, but stopping
/// short of each, so that it does not run under the letters. `None` if
/// they are too close for there to be any line between.
pub fn link(a: Rect, b: Rect) -> Option<(Point, Point)> {
    let (ca, cb) = (Point::new(a.x + a.w / 2.0, a.y + a.h / 2.0), Point::new(b.x + b.w / 2.0, b.y + b.h / 2.0));
    let (dx, dy) = (cb.x - ca.x, cb.y - ca.y);
    // How far along the line it leaves a rectangle, with a little room to spare.
    let leaves = |r: Rect| ((r.w / 2.0 + 4.0) / dx.abs().max(f32::MIN_POSITIVE)).min((r.h / 2.0 + 2.0) / dy.abs().max(f32::MIN_POSITIVE));
    let (ta, tb) = (leaves(a), leaves(b));
    (ta + tb < 1.0).then(|| (Point::new(ca.x + dx * ta, ca.y + dy * ta), Point::new(cb.x - dx * tb, cb.y - dy * tb)))
}

/// How large a word of a given weight is drawn.
pub fn size_for(weight: f32) -> f32 {
    (SIZES.0 + (SIZES.1 - SIZES.0) * weight.clamp(0.0, 1.0).powf(1.3)).round()
}

struct Spot {
    /// Which word, and where it is, relative to the widget.
    word: usize,
    rect: Rect,
    label: TextLayout,
}

#[derive(Default)]
struct CloudState {
    hover: Option<usize>,
}

pub struct Cloud<M> {
    words: Rc<Vec<Word>>,
    on_pick: Option<Box<dyn Fn(String) -> M>>,
    height: f32,
    spots: Vec<Spot>,
}

impl<M> Cloud<M> {
    pub fn new(words: Rc<Vec<Word>>) -> Self {
        Self { words, on_pick: None, height: 300.0, spots: vec![] }
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    /// Called with a word when it is clicked.
    pub fn on_pick(mut self, f: impl Fn(String) -> M + 'static) -> Self {
        self.on_pick = Some(Box::new(f));
        self
    }

    fn hit(&self, origin: Point, p: Point) -> Option<usize> {
        self.spots.iter().position(|s| Rect::new(origin.x + s.rect.x, origin.y + s.rect.y, s.rect.w, s.rect.h).inset(-2.0).contains(p))
    }
}

impl<M: 'static> Widget<M> for Cloud<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = Size::new(limits.max.w, self.height.min(limits.max.h));
        let base = cx.theme().text(TextRole::Strong).style();
        let labels: Vec<TextLayout> = self
            .words
            .iter()
            .map(|w| {
                let mut style = base;
                style.size = size_for(w.weight);
                style.weight = if w.weight > 0.45 { 700 } else { 600 };
                style.line_height = 1.1;
                cx.text().layout(&w.word, &style, None)
            })
            .collect();
        let places = place(&labels.iter().map(TextLayout::size).collect::<Vec<_>>(), size);
        self.spots = labels.into_iter().zip(places).enumerate().filter_map(|(word, (label, rect))| Some(Spot { word, rect: rect?, label })).collect();
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let p = cx.theme().palette();
        let hover = cx.state::<CloudState>().hover.filter(|i| *i < self.spots.len());
        // Shades between the accent and the text, so the cloud is of a
        // piece with the app whatever accent is chosen.
        let shades = [p.accent_text, p.text, p.accent_text.mix(p.text, 0.45), p.muted, p.accent_text.mix(p.muted, 0.5)];
        for (i, s) in self.spots.iter().enumerate() {
            let at = Point::new(b.x + s.rect.x, b.y + s.rect.y);
            let colour = if hover == Some(i) { p.accent_text } else { shades[s.word % shades.len()] };
            cx.scene.text(&s.label, at, colour);
            if hover == Some(i) {
                let y = (at.y + s.rect.h - 1.0).round();
                cx.scene.line(Point::new(at.x, y), Point::new(at.x + s.rect.w, y), (s.rect.h / 16.0).clamp(1.0, 2.5), colour);
            }
        }
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
                let st = cx.state::<CloudState>();
                if st.hover != hit {
                    st.hover = hit;
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerLeft => {
                cx.state::<CloudState>().hover = None;
                cx.request_redraw();
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } => match (self.hit(origin, *pos), &self.on_pick) {
                (Some(i), Some(pick)) => {
                    cx.emit(pick(self.words[self.spots[i].word].word.clone()));
                    Status::Captured
                }
                _ => Status::Ignored,
            },
            _ => Status::Ignored,
        }
    }
}

/// The picked words as a graph: each with its own small cloud of the
/// words that go with it, joined to it by edges. Clicking a word that
/// goes with one picks it too, and the graph grows; clicking a picked
/// word goes back to it.
pub struct Graph<M> {
    hubs: Rc<Vec<Hub>>,
    on_pick: Option<Box<dyn Fn(String) -> M>>,
    height: f32,
    arranged: Arranged,
    labels: Vec<TextLayout>,
}

impl<M> Graph<M> {
    pub fn new(hubs: Rc<Vec<Hub>>) -> Self {
        Self { hubs, on_pick: None, height: 300.0, arranged: Arranged::default(), labels: vec![] }
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    /// Called with a word when it is clicked.
    pub fn on_pick(mut self, f: impl Fn(String) -> M + 'static) -> Self {
        self.on_pick = Some(Box::new(f));
        self
    }

    fn hit(&self, origin: Point, p: Point) -> Option<usize> {
        self.arranged.nodes.iter().position(|n| Rect::new(origin.x + n.rect.x, origin.y + n.rect.y, n.rect.w, n.rect.h).inset(-3.0).contains(p))
    }
}

impl<M: 'static> Widget<M> for Graph<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = Size::new(limits.max.w, self.height.min(limits.max.h));
        let base = cx.theme().text(TextRole::Strong).style();
        let style = |hub: bool, weight: f32| {
            let mut style = base;
            style.size = if hub { HUB_SIZE } else { satellite_size(weight) };
            style.weight = if hub { 700 } else { 600 };
            style.line_height = 1.15;
            style
        };
        self.arranged = arrange(&self.hubs, &mut |word, hub, weight| cx.text().layout(word, &style(hub, weight), None).size(), size);
        self.labels = self.arranged.nodes.iter().map(|n| cx.text().layout(&n.word, &style(n.hub, n.weight), None)).collect();
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let p = cx.theme().palette();
        let nodes = &self.arranged.nodes;
        let hover = cx.state::<CloudState>().hover.filter(|i| *i < nodes.len());
        let moved = |r: Rect| Rect::new(b.x + r.x, b.y + r.y, r.w, r.h);
        for (from, to) in &self.arranged.edges {
            let Some((start, end)) = link(moved(nodes[*from].rect.inset(if nodes[*from].hub { -8.0 } else { 0.0 })), moved(nodes[*to].rect.inset(if nodes[*to].hub { -8.0 } else { 0.0 }))) else { continue };
            // Between two picked words the edge is the path taken; one the
            // pointer's word is on stands out from the rest.
            let (thick, colour) = if hover == Some(*from) || hover == Some(*to) {
                (2.0, p.accent)
            } else if nodes[*from].hub && nodes[*to].hub {
                (2.0, p.accent.with_alpha(0.6))
            } else {
                (1.0, p.muted.with_alpha(0.4))
            };
            cx.scene.line(start, end, thick, colour);
        }
        for (i, (node, label)) in nodes.iter().zip(&self.labels).enumerate() {
            let r = moved(node.rect);
            if node.hub {
                cx.scene.fill(r.inset(-8.0), 12.0, p.accent.with_alpha(0.16), None);
            }
            let colour = if node.hub || hover == Some(i) { p.accent_text } else { p.text.mix(p.muted, 1.0 - node.weight.clamp(0.0, 1.0)) };
            cx.scene.text(label, Point::new(r.x, r.y), colour);
        }
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
                let st = cx.state::<CloudState>();
                if st.hover != hit {
                    st.hover = hit;
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerLeft => {
                cx.state::<CloudState>().hover = None;
                cx.request_redraw();
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } => match (self.hit(origin, *pos), &self.on_pick) {
                (Some(i), Some(pick)) => {
                    cx.emit(pick(self.arranged.nodes[i].word.clone()));
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

    #[test]
    fn words_are_placed_from_the_middle_out_without_touching() {
        let sizes: Vec<Size> = (0..40).map(|i| Size::new(140.0 - i as f32 * 2.5, 44.0 - i as f32 * 0.7)).collect();
        let area = Size::new(900.0, 320.0);
        let rects = place(&sizes, area);
        let first = rects[0].unwrap();
        assert_eq!((first.x + first.w / 2.0, first.y + first.h / 2.0), (450.0, 160.0), "the weightiest in the middle");
        let placed: Vec<Rect> = rects.iter().flatten().copied().collect();
        assert!(placed.len() >= 30, "most find room: {}", placed.len());
        for (i, a) in placed.iter().enumerate() {
            assert!(a.x >= 0.0 && a.y >= 0.0 && a.right() <= area.w && a.bottom() <= area.h, "inside: {a:?}");
            for b in &placed[i + 1..] {
                assert!(a.right() <= b.x || b.right() <= a.x || a.bottom() <= b.y || b.bottom() <= a.y, "{a:?} and {b:?} overlap");
            }
        }
        // Later, smaller words are further out on the whole.
        let from_middle = |r: &Rect| ((r.x + r.w / 2.0 - 450.0).abs() / 450.0).max((r.y + r.h / 2.0 - 160.0).abs() / 160.0);
        assert!(placed[..5].iter().map(from_middle).sum::<f32>() < placed[placed.len() - 5..].iter().map(from_middle).sum::<f32>());
        // What cannot fit at all is left out, and what follows still placed.
        let odd = place(&[Size::new(50.0, 20.0), Size::new(500.0, 20.0), Size::new(40.0, 20.0)], Size::new(200.0, 100.0));
        assert!(odd[0].is_some() && odd[1].is_none() && odd[2].is_some());
        assert_eq!(place(&[], area), vec![]);
        assert_eq!(place(&[Size::new(10.0, 10.0)], Size::new(0.0, 0.0)), vec![None]);
    }

    fn hub(word: &str, related: &[(&str, f32)]) -> Hub {
        Hub { word: word.into(), related: related.iter().map(|(w, weight)| ((*w).to_owned(), *weight)).collect() }
    }

    #[test]
    fn picked_words_are_joined_to_each_other_and_to_the_words_that_go_with_them() {
        let hubs = [hub("dog", &[("beach", 1.0), ("park", 0.6), ("sea", 0.4), ("hiking", 0.3)]), hub("beach", &[("sea", 1.0), ("dog", 0.9), ("sand", 0.5), ("kite", 0.2)])];
        let area = Size::new(840.0, 320.0);
        let g = arrange(&hubs, &mut |word, hub, _| Size::new(word.len() as f32 * if hub { 16.0 } else { 9.0 }, if hub { 32.0 } else { 18.0 }), area);
        let at = |word: &str| g.nodes.iter().position(|n| n.word == word).unwrap_or_else(|| panic!("{word} is in the graph"));
        let joined = |a: &str, b: &str| g.edges.contains(&(at(a).min(at(b)), at(a).max(at(b))));
        assert_eq!(g.nodes.iter().filter(|n| n.hub).map(|n| n.word.as_str()).collect::<Vec<_>>(), ["dog", "beach"]);
        assert_eq!(g.nodes.len(), 7, "each word once, though sea goes with both and each picked word with the other");
        assert!(joined("dog", "beach") && joined("dog", "park") && joined("dog", "hiking") && joined("beach", "sand") && joined("beach", "kite"));
        assert!(joined("dog", "sea") && joined("beach", "sea"), "a word that goes with both is joined to both");
        assert!(!joined("park", "sand") && !joined("dog", "sand"));
        assert_eq!(g.edges.len(), 7, "and no edge twice");
        // The picked words side by side in the order picked, the rest round them.
        let middle = |word: &str| (g.nodes[at(word)].rect.x + g.nodes[at(word)].rect.w / 2.0, g.nodes[at(word)].rect.y + g.nodes[at(word)].rect.h / 2.0);
        assert_eq!((middle("dog"), middle("beach")), ((210.0, 160.0), (630.0, 160.0)));
        assert!(middle("park").0 < 420.0 && middle("sand").0 > 420.0, "each by its own picked word");
        assert!((middle("sea").0 - 420.0).abs() < 90.0, "and one that goes with both between them: {:?}", middle("sea"));
        for (i, a) in g.nodes.iter().enumerate() {
            assert!(a.rect.x >= 0.0 && a.rect.y >= 0.0 && a.rect.right() <= area.w && a.rect.bottom() <= area.h, "inside: {a:?}");
            for b in &g.nodes[i + 1..] {
                assert!(a.rect.right() <= b.rect.x || b.rect.right() <= a.rect.x || a.rect.bottom() <= b.rect.y || b.rect.bottom() <= a.rect.y, "{a:?} and {b:?} overlap");
            }
        }
        assert_eq!(arrange(&[], &mut |_, _, _| Size::new(1.0, 1.0), area), Arranged::default());
        // One picked word alone sits in the middle.
        let one = arrange(&hubs[..1], &mut |_, _, _| Size::new(40.0, 20.0), area);
        assert_eq!((one.nodes[0].rect.x, one.nodes.len(), one.edges.len()), (400.0, 5, 4));
    }

    #[test]
    fn an_edge_stops_short_of_the_words_it_joins() {
        let (a, b) = (Rect::new(0.0, 0.0, 100.0, 20.0), Rect::new(300.0, 0.0, 60.0, 20.0));
        let (start, end) = link(a, b).unwrap();
        assert_eq!((start, end), (Point::new(104.0, 10.0), Point::new(296.0, 10.0)), "from just past one to just before the other");
        let (start, end) = link(Rect::new(0.0, 0.0, 40.0, 20.0), Rect::new(0.0, 200.0, 40.0, 20.0)).unwrap();
        assert_eq!((start, end), (Point::new(20.0, 22.0), Point::new(20.0, 198.0)));
        assert_eq!(link(a, Rect::new(90.0, 5.0, 60.0, 20.0)), None, "no line between two that touch");
        assert!(satellite_size(1.0) > satellite_size(0.0) && satellite_size(1.0) < HUB_SIZE);
    }

    #[test]
    fn weightier_words_are_larger() {
        assert_eq!((size_for(0.0), size_for(1.0), size_for(7.0)), (SIZES.0, SIZES.1, SIZES.1));
        assert!(size_for(0.5) > size_for(0.2) && size_for(0.5) < 32.0);
    }
}
