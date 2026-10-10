//! A workflow drawn as what it is: boxes for its nodes, and lines from
//! each to those it leads to.
//!
//! A box is dragged to move it. Its right edge has a port: dragged from
//! there to another box, the two are joined. A click chooses a box; a
//! click on nothing chooses none.

use neo::prelude::*;
use neo::{CursorIcon, Cx, DrawCx, Event, EventCx, Limits, Point, PointerButton, Rect, Size, Status, TextLayout, Widget};

/// How a node's work stands, while its workflow runs or after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Running,
    Done,
    Failed,
}

/// A node as it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub id: u32,
    pub title: String,
    /// What kind it is, and the model it uses if it has one of its own.
    pub under: String,
    pub at: Point,
    pub stage: Option<Stage>,
    /// Whether anything can lead on from it, and so whether it has a port.
    pub leads: bool,
}

/// What is done to a workflow in its picture.
#[derive(Clone, Debug, PartialEq)]
pub enum FlowEvent {
    Chose(Option<u32>),
    Moved(u32, Point),
    /// The first is to lead to the second.
    Joined(u32, u32),
}

/// The size of a node's box, and of the port on its edge.
pub const BOX: Size = Size { w: 168.0, h: 58.0 };
const PORT: f32 = 7.0;
const SLACK: f32 = 3.0;

#[derive(Default)]
struct ViewState {
    /// A box being dragged: which, where the pointer took hold of it, and how far it has gone.
    moving: Option<(u32, Point, Point)>,
    /// A line being drawn out of a box's port, to wherever the pointer is.
    joining: Option<(u32, Point)>,
    hovered: Option<u32>,
    /// How far the whole picture has been pushed about, and the push under way: where it began.
    pan: Point,
    panning: Option<(Point, Point)>,
}

/// Shows a workflow: see [`flow_view`].
pub struct FlowView<M> {
    nodes: Vec<Shown>,
    edges: Vec<(u32, u32)>,
    selected: Option<u32>,
    on: Box<dyn Fn(FlowEvent) -> M>,
    titles: Vec<(TextLayout, TextLayout)>,
}

/// A picture of `nodes` and the joins between them, with one chosen,
/// that says what is done to it.
pub fn flow_view<M>(nodes: Vec<Shown>, edges: Vec<(u32, u32)>, selected: Option<u32>, on: impl Fn(FlowEvent) -> M + 'static) -> FlowView<M> {
    FlowView { nodes, edges, selected, on: Box::new(on), titles: vec![] }
}

impl<M> FlowView<M> {
    fn rect_of(&self, origin: Point, n: &Shown, moving: Option<(u32, Point, Point)>) -> Rect {
        let at = match moving {
            Some((id, _, by)) if id == n.id => Point::new((n.at.x + by.x).max(0.0), (n.at.y + by.y).max(0.0)),
            _ => n.at,
        };
        Rect::new(origin.x + at.x, origin.y + at.y, BOX.w, BOX.h)
    }

    fn port(r: Rect) -> Point {
        Point::new(r.right(), r.y + r.h / 2.0)
    }

    /// The node whose box is at `p`, the topmost if they overlap.
    fn at(&self, origin: Point, p: Point) -> Option<&Shown> {
        self.nodes.iter().rev().find(|n| self.rect_of(origin, n, None).contains(p))
    }
}

/// The points of a line that leaves one place going right and arrives at
/// another from the left, bending between.
fn wire(from: Point, to: Point) -> Vec<Point> {
    let reach = ((to.x - from.x).abs() * 0.5).max(40.0);
    let (a, b) = (Point::new(from.x + reach, from.y), Point::new(to.x - reach, to.y));
    (0..=24)
        .map(|i| {
            let t = i as f32 / 24.0;
            let u = 1.0 - t;
            let at = |p0: f32, p1: f32, p2: f32, p3: f32| u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3;
            Point::new(at(from.x, a.x, b.x, to.x), at(from.y, a.y, b.y, to.y))
        })
        .collect()
}

impl<M: 'static> Widget<M> for FlowView<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let (strong, small) = (cx.theme().text(TextRole::Strong).style(), cx.theme().text(TextRole::Caption).style());
        let inner = Some(BOX.w - 30.0);
        self.titles = self.nodes.iter().map(|n| (cx.text().layout(&n.title, &strong, inner), cx.text().layout(&n.under, &small, inner))).collect();
        limits.max
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let (moving, joining, hovered) = {
            let st = cx.state::<ViewState>();
            (st.moving, st.joining, st.hovered)
        };
        cx.scene.push_clip(b);
        cx.scene.paint(b, theme.small_radius(), &theme.paint(Surface::Inset));
        let pan = cx.state::<ViewState>().pan;
        let origin = Point::new(b.x + pan.x, b.y + pan.y);
        let rect = |n: &Shown| self.rect_of(origin, n, moving);
        // The joins first, under the boxes they join.
        for (from, to) in &self.edges {
            let (Some(a), Some(z)) = (self.nodes.iter().find(|n| n.id == *from), self.nodes.iter().find(|n| n.id == *to)) else { continue };
            let (ra, rz) = (rect(a), rect(z));
            let chosen = self.selected == Some(*from) || self.selected == Some(*to);
            cx.scene.polyline(&wire(Self::port(ra), Point::new(rz.x, rz.y + rz.h / 2.0)), if chosen { 2.0 } else { 1.5 }, if chosen { p.accent } else { p.faint });
        }
        if let Some((from, to)) = joining
            && let Some(a) = self.nodes.iter().find(|n| n.id == from)
        {
            cx.scene.polyline(&wire(Self::port(rect(a)), to), 2.0, p.accent);
        }
        for (n, (title, under)) in self.nodes.iter().zip(&self.titles) {
            let r = rect(n);
            // Over what was drawn before it, lines and other boxes' words alike.
            cx.scene.push_layer();
            let chosen = self.selected == Some(n.id);
            cx.scene.paint(r, theme.small_radius(), &theme.paint(if chosen { Surface::Raised } else { Surface::Card }));
            if chosen {
                cx.scene.fill(r, theme.small_radius(), neo::Color::TRANSPARENT, Some((2.0, p.accent)));
            }
            cx.scene.push_clip(r.inset(4.0));
            cx.scene.text(title, Point::new(r.x + 12.0, r.y + 9.0), p.text);
            cx.scene.text(under, Point::new(r.x + 12.0, r.y + 31.0), p.muted);
            cx.scene.pop_clip();
            // How its work stands, in the corner.
            if let Some(stage) = n.stage {
                let color = match stage {
                    Stage::Running => p.accent,
                    Stage::Done => p.good,
                    Stage::Failed => p.bad,
                };
                cx.scene.fill(Rect::new(r.right() - 18.0, r.y + 10.0, 8.0, 8.0), 4.0, color, None);
            }
            // Where it is joined to what it leads to, and where it is joined from.
            if n.leads {
                let port = Self::port(r);
                let near = hovered == Some(n.id) || joining.is_some_and(|(from, _)| from == n.id);
                cx.scene.fill(Rect::new(port.x - PORT, port.y - PORT, PORT * 2.0, PORT * 2.0), PORT, if near { p.accent } else { p.surface }, Some((1.5, if near { p.accent } else { p.faint })));
            }
        }
        if self.nodes.is_empty() {
            let hint = cx.theme().text(TextRole::Body).style();
            let words = cx.text().layout("Nothing here yet. Add a Question, an Agent and an Answer, and join them.", &hint, Some(b.w - 40.0));
            cx.scene.text(&words, Point::new(b.x + 20.0, b.y + 20.0), p.muted);
        }
        cx.scene.pop_clip();
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let pan = cx.state::<ViewState>().pan;
        let origin = Point::new(b.x + pan.x, b.y + pan.y);
        let on_port = |p: Point| self.nodes.iter().rev().find(|n| n.leads && {
            let port = Self::port(self.rect_of(origin, n, None));
            (p.x - port.x).abs() <= PORT + 3.0 && (p.y - port.y).abs() <= PORT + 3.0
        });
        match event {
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                if let Some(n) = on_port(*pos) {
                    cx.state::<ViewState>().joining = Some((n.id, *pos));
                    return Status::Captured;
                }
                match self.at(origin, *pos) {
                    Some(n) => {
                        cx.state::<ViewState>().moving = Some((n.id, *pos, Point::ZERO));
                        if self.selected != Some(n.id) {
                            cx.emit((self.on)(FlowEvent::Chose(Some(n.id))));
                        }
                    }
                    // On nothing: none is chosen, and the whole picture is pushed about from here.
                    None => {
                        cx.state::<ViewState>().panning = Some((*pos, pan));
                        if self.selected.is_some() {
                            cx.emit((self.on)(FlowEvent::Chose(None)));
                        }
                    }
                }
                Status::Captured
            }
            Event::PointerMoved { pos } => {
                let st = cx.state::<ViewState>();
                if let Some((_, to)) = &mut st.joining {
                    *to = *pos;
                    cx.set_cursor(CursorIcon::Crosshair);
                    cx.request_redraw();
                    return Status::Captured;
                }
                if let Some((_, from, by)) = &mut st.moving {
                    *by = Point::new(pos.x - from.x, pos.y - from.y);
                    cx.set_cursor(CursorIcon::Grabbing);
                    cx.request_redraw();
                    return Status::Captured;
                }
                if let Some((from, was)) = st.panning {
                    st.pan = Point::new(was.x + pos.x - from.x, was.y + pos.y - from.y);
                    cx.set_cursor(CursorIcon::Grabbing);
                    cx.request_redraw();
                    return Status::Captured;
                }
                let over = on_port(*pos).map(|n| n.id);
                if over.is_some() {
                    cx.set_cursor(CursorIcon::Crosshair);
                }
                if cx.state::<ViewState>().hovered != over {
                    cx.state::<ViewState>().hovered = over;
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerReleased { pos, button: PointerButton::Primary } => {
                let st = cx.state::<ViewState>();
                let (joining, moving) = (st.joining.take(), st.moving.take());
                if st.panning.take().is_some() {
                    return Status::Captured;
                }
                if let Some((from, _)) = joining {
                    cx.request_redraw();
                    if let Some(to) = self.at(origin, *pos).filter(|n| n.id != from) {
                        cx.emit((self.on)(FlowEvent::Joined(from, to.id)));
                    }
                    return Status::Captured;
                }
                if let Some((id, _, by)) = moving {
                    cx.request_redraw();
                    if (by.x.abs() > SLACK || by.y.abs() > SLACK)
                        && let Some(n) = self.nodes.iter().find(|n| n.id == id)
                    {
                        cx.emit((self.on)(FlowEvent::Moved(id, Point::new((n.at.x + by.x).max(0.0).round(), (n.at.y + by.y).max(0.0).round()))));
                    }
                    return Status::Captured;
                }
                Status::Ignored
            }
            // The wheel pushes it about too, for a workflow larger than its place.
            Event::Wheel { pos, delta } if b.contains(*pos) => {
                let st = cx.state::<ViewState>();
                st.pan = Point::new(st.pan.x + delta.x, st.pan.y + delta.y);
                cx.request_redraw();
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo::testing::Harness;

    struct Graph {
        nodes: Vec<Shown>,
        edges: Vec<(u32, u32)>,
        chosen: Option<u32>,
        heard: Vec<FlowEvent>,
    }

    impl App for Graph {
        type Message = FlowEvent;

        fn update(&mut self, e: FlowEvent) {
            match &e {
                FlowEvent::Chose(id) => self.chosen = *id,
                FlowEvent::Moved(id, to) => self.nodes.iter_mut().filter(|n| n.id == *id).for_each(|n| n.at = *to),
                FlowEvent::Joined(a, b) => self.edges.push((*a, *b)),
            }
            self.heard.push(e);
        }

        fn view(&self) -> Element<FlowEvent> {
            Element::new(flow_view(self.nodes.clone(), self.edges.clone(), self.chosen, |e| e))
        }

        fn window(&self) -> neo::WindowSettings {
            neo::WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
    }

    fn drag(h: &mut Harness<Graph>, from: Point, to: Point) -> Vec<u8> {
        h.event(Event::PointerMoved { pos: from });
        h.event(Event::PointerPressed { pos: from, button: PointerButton::Primary });
        h.event(Event::PointerMoved { pos: Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0) });
        h.event(Event::PointerMoved { pos: to });
        let during = h.render(1.0);
        h.event(Event::PointerReleased { pos: to, button: PointerButton::Primary });
        h.render(1.0);
        during
    }

    #[test]
    fn boxes_are_chosen_moved_and_joined_by_dragging() {
        let node = |id, title: &str, x, y, leads| Shown { id, title: title.into(), under: "Agent".into(), at: Point::new(x, y), stage: None, leads };
        let graph = Graph { nodes: vec![node(1, "Question", 20.0, 20.0, true), node(2, "Agent", 260.0, 20.0, true), node(3, "Answer", 500.0, 140.0, false)], edges: vec![(1, 2)], chosen: None, heard: vec![] };
        let mut h = Harness::new(graph, Size::new(760.0, 320.0)).expect("a GPU adapter is required for these tests");
        let empty = h.render(1.0);
        // A click on a box chooses it; one on nothing chooses none; the same again says nothing more.
        h.click(Point::new(300.0, 40.0));
        h.click(Point::new(300.0, 40.0));
        assert_eq!((h.app().chosen, h.app().heard.len()), (Some(2), 1));
        assert!(h.render(1.0) != empty, "the one chosen is marked");
        h.click(Point::new(300.0, 250.0));
        assert_eq!(h.app().heard.last(), Some(&FlowEvent::Chose(None)));
        // Dragged by its body, a box moves, and is where it was let go; its joins follow as it goes.
        let before = h.render(1.0);
        let during = drag(&mut h, Point::new(300.0, 40.0), Point::new(340.0, 160.0));
        assert!(during != before);
        assert_eq!(h.app().heard.last(), Some(&FlowEvent::Moved(2, Point::new(300.0, 140.0))));
        // Not past the top left corner.
        drag(&mut h, Point::new(60.0, 40.0), Point::new(-200.0, -200.0));
        assert_eq!(h.app().nodes[0].at, Point::ZERO);
        // From the dot on a box's right edge to another box: the two are joined. Let go on nothing: not.
        let port = Point::new(300.0 + BOX.w, 140.0 + BOX.h / 2.0);
        let wiring = drag(&mut h, port, Point::new(560.0, 170.0));
        assert_eq!(h.app().heard.last(), Some(&FlowEvent::Joined(2, 3)));
        assert!(wiring != h.render(1.0), "a line follows the pointer until it is let go");
        let heard = h.app().heard.len();
        drag(&mut h, port, Point::new(600.0, 290.0));
        drag(&mut h, port, port);
        assert_eq!((h.app().heard.len(), h.app().edges.clone()), (heard, vec![(1, 2), (2, 3)]));
        // The answer leads nowhere, so it has no dot: dragging from its edge moves it.
        drag(&mut h, Point::new(500.0 + BOX.w - 2.0, 140.0 + BOX.h / 2.0), Point::new(520.0 + BOX.w, 150.0 + BOX.h / 2.0));
        assert!(matches!(h.app().heard.last(), Some(FlowEvent::Moved(3, _))), "{:?}", h.app().heard.last());
        // Dragged by nothing, the whole picture moves, and a box is then found where it is shown.
        drag(&mut h, Point::new(700.0, 300.0), Point::new(680.0, 260.0));
        let heard = h.app().heard.len();
        h.click(Point::new(40.0 - 20.0, 30.0 - 40.0 + 20.0));
        assert_eq!((h.app().heard.len(), h.app().chosen), (heard + 1, Some(1)), "{:?}", h.app().heard.last());
        // How each node's work stands is shown on it.
        let idle = h.render(1.0);
        h.app_mut().nodes[1].stage = Some(Stage::Running);
        let running = h.render(1.0);
        h.app_mut().nodes[1].stage = Some(Stage::Failed);
        assert!(idle != running && running != h.render(1.0));
    }
}
