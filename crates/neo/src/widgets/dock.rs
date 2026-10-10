//! Panels that share a window: side by side, one above another, or behind
//! one another as tabs, and rearranged by dragging.
//!
//! How they are arranged is a value of its own, a [`Dock`], which the app
//! keeps: the [`dock`] widget shows it and says what it would be after
//! whatever the user did, and the app keeps that instead. It is written
//! as a line of text, to be kept in a settings file and read back.

use armature_render::{Point, Rect, Size, TextLayout};
use neo_theme::{Surface, TextRole};

use crate::ThemeCx;
use crate::core::{Cx, CursorIcon, DrawCx, Element, EventCx, Length, Limits, Widget};
use armature::ResizeEdge;
use crate::event::{Event, PointerButton, Status};

/// An arrangement of panels, each known by a name of the app's choosing.
#[derive(Clone, Debug, PartialEq)]
pub enum Dock {
    /// Panels in one place, one of them in front: `active` is which.
    Tabs { panels: Vec<String>, active: usize },
    /// Two arrangements sharing a place: side by side if `across`, else
    /// one above the other. `share` is how much of it the first has.
    Split { across: bool, share: f32, first: Box<Dock>, second: Box<Dock> },
}

/// Where a panel is put, against the group of tabs it is dropped on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
    /// Among the group's own tabs.
    Middle,
}

/// The least and most of a split its first part may have.
const SHARES: (f32, f32) = (0.08, 0.92);

impl Dock {
    /// Panels behind one another, the first in front.
    pub fn tabs<S: Into<String>>(panels: impl IntoIterator<Item = S>) -> Self {
        Dock::Tabs { panels: panels.into_iter().map(Into::into).collect(), active: 0 }
    }

    /// `first` to the left of `second`, with `share` of the width.
    pub fn beside(first: Dock, share: f32, second: Dock) -> Self {
        Dock::Split { across: true, share: share.clamp(SHARES.0, SHARES.1), first: Box::new(first), second: Box::new(second) }
    }

    /// `first` above `second`, with `share` of the height.
    pub fn above(first: Dock, share: f32, second: Dock) -> Self {
        Dock::Split { across: false, share: share.clamp(SHARES.0, SHARES.1), first: Box::new(first), second: Box::new(second) }
    }

    /// Every panel there is, in reading order.
    pub fn panels(&self) -> Vec<&str> {
        match self {
            Dock::Tabs { panels, .. } => panels.iter().map(String::as_str).collect(),
            Dock::Split { first, second, .. } => first.panels().into_iter().chain(second.panels()).collect(),
        }
    }

    pub fn contains(&self, panel: &str) -> bool {
        self.panels().contains(&panel)
    }

    /// The panels that are in front, one to each group of tabs.
    pub fn shown(&self) -> Vec<&str> {
        match self {
            Dock::Tabs { panels, active } => panels.get(*active).or(panels.first()).map(String::as_str).into_iter().collect(),
            Dock::Split { first, second, .. } => first.shown().into_iter().chain(second.shown()).collect(),
        }
    }

    /// Brings a panel to the front of its group.
    pub fn show(&mut self, panel: &str) {
        match self {
            Dock::Tabs { panels, active } => {
                if let Some(i) = panels.iter().position(|p| p == panel) {
                    *active = i;
                }
            }
            Dock::Split { first, second, .. } => {
                first.show(panel);
                second.show(panel);
            }
        }
    }

    /// Without a panel: its group closes up, and a group left empty gives
    /// its place to what was beside it. Nothing, if it was the only one.
    pub fn without(self, panel: &str) -> Option<Dock> {
        match self {
            Dock::Tabs { mut panels, active } => {
                let Some(at) = panels.iter().position(|p| p == panel) else { return Some(Dock::Tabs { panels, active }) };
                panels.remove(at);
                // The one in front stays in front; closed itself, its neighbour comes forward.
                let active = if at < active { active - 1 } else { active.min(panels.len().saturating_sub(1)) };
                (!panels.is_empty()).then_some(Dock::Tabs { panels, active })
            }
            Dock::Split { across, share, first, second } => match (first.without(panel), second.without(panel)) {
                (Some(first), Some(second)) => Some(Dock::Split { across, share, first: Box::new(first), second: Box::new(second) }),
                (one, other) => one.or(other),
            },
        }
    }

    /// With `panel` put against the group that `onto` is in: among its
    /// tabs, or in a new group to one side of it. A panel already there is
    /// moved; one that is new is added. With `onto` not there, or the
    /// panel itself and nothing else in its group to be beside, unchanged.
    pub fn with(self, panel: &str, onto: &str, side: Side) -> Dock {
        let same_group = self.group_of(panel).is_some_and(|g| g.iter().any(|p| p == onto));
        if !self.contains(onto) || (panel == onto && side == Side::Middle) || (same_group && self.group_of(panel).is_some_and(|g| g.len() == 1)) {
            return self;
        }
        // Dropped on its own tabs as a tab, it only comes to the front.
        if same_group && side == Side::Middle {
            let mut d = self;
            d.show(panel);
            return d;
        }
        // Out of where it was, then in where it goes. `onto` is still there:
        // it is another panel, or one of several in the group.
        let anchor = if panel == onto { self.group_of(panel).and_then(|g| g.iter().find(|p| *p != panel).cloned()).unwrap_or_default() } else { onto.to_owned() };
        let Some(rest) = self.without(panel) else { return Dock::tabs([panel]) };
        rest.put(panel, &anchor, side)
    }

    fn group_of(&self, panel: &str) -> Option<&Vec<String>> {
        match self {
            Dock::Tabs { panels, .. } => panels.iter().any(|p| p == panel).then_some(panels),
            Dock::Split { first, second, .. } => first.group_of(panel).or_else(|| second.group_of(panel)),
        }
    }

    fn put(self, panel: &str, onto: &str, side: Side) -> Dock {
        match self {
            Dock::Tabs { mut panels, active } if panels.iter().any(|p| p == onto) => {
                let new = Dock::tabs([panel]);
                match side {
                    Side::Middle => {
                        panels.push(panel.to_owned());
                        Dock::Tabs { active: panels.len() - 1, panels }
                    }
                    Side::Left => Dock::beside(new, 0.5, Dock::Tabs { panels, active }),
                    Side::Right => Dock::beside(Dock::Tabs { panels, active }, 0.5, new),
                    Side::Top => Dock::above(new, 0.5, Dock::Tabs { panels, active }),
                    Side::Bottom => Dock::above(Dock::Tabs { panels, active }, 0.5, new),
                }
            }
            tabs @ Dock::Tabs { .. } => tabs,
            Dock::Split { across, share, first, second } => Dock::Split { across, share, first: Box::new(first.put(panel, onto, side)), second: Box::new(second.put(panel, onto, side)) },
        }
    }

    /// With the split that `path` leads to given a new share: false for
    /// the first part at each split on the way down, true for the second.
    fn with_share(mut self, path: &[bool], share: f32) -> Dock {
        let mut at = &mut self;
        for second in path {
            let Dock::Split { first, second: other, .. } = at else { return self };
            at = if *second { other } else { first };
        }
        if let Dock::Split { share: s, .. } = at {
            *s = share.clamp(SHARES.0, SHARES.1);
        }
        self
    }

    /// As a line of text: `row(0.25, tabs(*scene, assets), column(0.7,
    /// tabs(*view), tabs(*console, log)))`. The star marks the panel in
    /// front. Names may hold anything but commas, brackets and stars.
    pub fn encode(&self) -> String {
        match self {
            Dock::Tabs { panels, active } => format!("tabs({})", panels.iter().enumerate().map(|(i, p)| if i == *active { format!("*{p}") } else { p.clone() }).collect::<Vec<_>>().join(", ")),
            Dock::Split { across, share, first, second } => format!("{}({}, {}, {})", if *across { "row" } else { "column" }, (share * 1000.0).round() / 1000.0, first.encode(), second.encode()),
        }
    }

    /// Reads what [`encode`](Self::encode) wrote, or nothing if it is not that.
    pub fn parse(text: &str) -> Option<Dock> {
        let (dock, rest) = Self::parse_one(text.trim())?;
        rest.trim().is_empty().then_some(dock)
    }

    fn parse_one(text: &str) -> Option<(Dock, &str)> {
        let (kind, rest) = text.split_once('(')?;
        match kind.trim() {
            "tabs" => {
                let (inside, rest) = rest.split_once(')')?;
                let names: Vec<&str> = inside.split(',').map(str::trim).filter(|n| !n.is_empty()).collect();
                let active = names.iter().position(|n| n.starts_with('*')).unwrap_or(0);
                let panels: Vec<String> = names.iter().map(|n| n.trim_start_matches('*').trim().to_owned()).collect();
                (!panels.is_empty() && panels.iter().all(|p| !p.is_empty() && !p.contains(['(', ')', '*']))).then_some((Dock::Tabs { panels, active }, rest))
            }
            kind @ ("row" | "column") => {
                let (share, rest) = rest.split_once(',')?;
                let share: f32 = share.trim().parse().ok().filter(|s: &f32| s.is_finite())?;
                let (first, rest) = Self::parse_one(rest.trim_start())?;
                let (second, rest) = Self::parse_one(rest.trim_start().strip_prefix(',')?.trim_start())?;
                let split = if kind == "row" { Dock::beside(first, share, second) } else { Dock::above(first, share, second) };
                Some((split, rest.trim_start().strip_prefix(')')?))
            }
            _ => None,
        }
    }
}

/// The height of a group's strip of tabs, the room between groups, and
/// how far a tab is dragged before it is being moved and not clicked.
const STRIP: f32 = 30.0;
const GAP: f32 = 5.0;
/// The same, flat: a lower strip, and a hairline between groups.
const FLAT_STRIP: f32 = 25.0;
const FLAT_GAP: f32 = 1.0;

/// How a dock's tabs and the divisions between its groups are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabStyle {
    /// Each group a card with a strip of tabs set into it, the one in
    /// front a raised pill, and room between one group and the next.
    #[default]
    Raised,
    /// Lines and no surfaces: tabs are plain rectangles side by side with
    /// a line between them and a line under the strip, the one in front
    /// marked along its top and open to its panel below; groups meet at a
    /// hairline. For an app of many panels, where edges should be quiet.
    Flat,
}
const TAB_X: f32 = 12.0;
const SLACK: f32 = 6.0;

/// A group of tabs as it is laid out.
struct Group {
    panels: Vec<String>,
    active: usize,
    /// The whole group, its strip of tabs and each tab in it, from the dock's own corner.
    rect: Rect,
    tabs: Vec<Rect>,
    titles: Vec<TextLayout>,
}

/// The bar between the two parts of a split, to drag.
struct Bar {
    path: Vec<bool>,
    rect: Rect,
    across: bool,
    /// The place the split has to share, along the way it is split.
    span: (f32, f32),
}

#[derive(Default)]
struct DockState {
    /// A tab pressed, and where; `moving` once it has been dragged far enough.
    pressed: Option<(String, Point)>,
    moving: bool,
    pointer: Point,
    /// A bar being dragged, by the way to its split.
    bar: Option<Vec<bool>>,
    hovered_bar: Option<Vec<bool>>,
}

/// Shows a [`Dock`]: see [`dock`].
pub struct DockView<M> {
    layout: Dock,
    titles: Vec<(String, String)>,
    /// The content of each panel in front, in the order [`Dock::shown`] gives.
    children: Vec<Element<M>>,
    on_change: Box<dyn Fn(Dock) -> M>,
    groups: Vec<Group>,
    bars: Vec<Bar>,
    style: TabStyle,
}

/// Panels arranged as `layout` says, filling what room there is. `title`
/// names a panel on its tab; `content` is asked for each panel that is in
/// front; `on_change` is given the arrangement as it would be after a tab
/// is chosen, a bar is dragged or a tab is dragged to another place, for
/// the app to keep and show next.
pub fn dock<M>(layout: &Dock, title: impl Fn(&str) -> String, mut content: impl FnMut(&str) -> Element<M>, on_change: impl Fn(Dock) -> M + 'static) -> DockView<M> {
    DockView { titles: layout.panels().into_iter().map(|p| (p.to_owned(), title(p))).collect(), children: layout.shown().into_iter().map(&mut content).collect(), layout: layout.clone(), on_change: Box::new(on_change), groups: vec![], bars: vec![], style: TabStyle::Raised }
}

impl<M> DockView<M> {
    /// How the tabs are drawn: see [`TabStyle`].
    pub fn tabs(mut self, style: TabStyle) -> Self {
        self.style = style;
        self
    }

    fn strip(&self) -> f32 {
        if self.style == TabStyle::Flat { FLAT_STRIP } else { STRIP }
    }

    fn gap(&self) -> f32 {
        if self.style == TabStyle::Flat { FLAT_GAP } else { GAP }
    }

    /// Lays the arrangement out in `rect`, noting each group and bar.
    fn arrange(&mut self, cx: &mut Cx, node: &Dock, rect: Rect, path: &mut Vec<bool>) {
        match node {
            Dock::Tabs { panels, active } => {
                let style = cx.theme().text(TextRole::Caption).style();
                let titles: Vec<TextLayout> = panels.iter().map(|p| cx.text().layout(self.titles.iter().find(|(id, _)| id == p).map_or(p.as_str(), |(_, t)| t.as_str()), &style, None)).collect();
                // Tabs as wide as their names, made narrower together if there is not the room.
                let wanted: f32 = titles.iter().map(|t| t.size().w + TAB_X * 2.0).sum();
                let squeeze = (rect.w / wanted.max(1.0)).min(1.0);
                let (mut x, strip) = (rect.x, self.strip());
                let tabs = titles
                    .iter()
                    .map(|t| {
                        let w = ((t.size().w + TAB_X * 2.0) * squeeze).floor();
                        let r = Rect::new(x, rect.y, w, strip);
                        x += w;
                        r
                    })
                    .collect();
                self.groups.push(Group { panels: panels.clone(), active: (*active).min(panels.len().saturating_sub(1)), rect, tabs, titles });
            }
            Dock::Split { across, share, first, second } => {
                let gap = self.gap();
                let whole = if *across { rect.w } else { rect.h };
                let a = ((whole - gap) * share).round().max(0.0);
                let (one, bar, two) = if *across {
                    (Rect::new(rect.x, rect.y, a, rect.h), Rect::new(rect.x + a, rect.y, gap, rect.h), Rect::new(rect.x + a + gap, rect.y, (rect.w - a - gap).max(0.0), rect.h))
                } else {
                    (Rect::new(rect.x, rect.y, rect.w, a), Rect::new(rect.x, rect.y + a, rect.w, gap), Rect::new(rect.x, rect.y + a + gap, rect.w, (rect.h - a - gap).max(0.0)))
                };
                self.bars.push(Bar { path: path.clone(), rect: bar, across: *across, span: if *across { (rect.x, rect.w) } else { (rect.y, rect.h) } });
                path.push(false);
                self.arrange(cx, first, one, path);
                *path.last_mut().expect("just pushed") = true;
                self.arrange(cx, second, two, path);
                path.pop();
            }
        }
    }

    /// Where a tab dragged to `p` would go: onto which group, and which side of it.
    fn target(&self, origin: Point, p: Point) -> Option<(usize, Side)> {
        let (i, g) = self.groups.iter().enumerate().find(|(_, g)| g.rect.translate(origin).contains(p))?;
        let r = g.rect.translate(origin);
        // On the tabs themselves, or in the middle: among them. Near an edge: to that side.
        let (fx, fy) = ((p.x - r.x) / r.w.max(1.0), (p.y - r.y) / r.h.max(1.0));
        let side = if p.y < r.y + self.strip() {
            Side::Middle
        } else if fx < 0.25 {
            Side::Left
        } else if fx > 0.75 {
            Side::Right
        } else if fy < 0.3 {
            Side::Top
        } else if fy > 0.7 {
            Side::Bottom
        } else {
            Side::Middle
        };
        Some((i, side))
    }
}

/// The part of a group a panel dropped on `side` of it would take.
fn landing(r: Rect, side: Side) -> Rect {
    match side {
        Side::Left => Rect::new(r.x, r.y, r.w / 2.0, r.h),
        Side::Right => Rect::new(r.x + r.w / 2.0, r.y, r.w / 2.0, r.h),
        Side::Top => Rect::new(r.x, r.y, r.w, r.h / 2.0),
        Side::Bottom => Rect::new(r.x, r.y + r.h / 2.0, r.w, r.h / 2.0),
        Side::Middle => r,
    }
}

impl<M: 'static> Widget<M> for DockView<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.children
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = limits.max;
        (self.groups, self.bars) = (vec![], vec![]);
        let layout = self.layout.clone();
        self.arrange(cx, &layout, Rect::new(0.0, 0.0, size.w, size.h), &mut vec![]);
        // Each panel in front has its group, less the strip of tabs.
        let strip = self.strip();
        for (child, g) in self.children.iter_mut().zip(&self.groups) {
            let room = Size::new(g.rect.w, (g.rect.h - strip).max(0.0));
            child.layout(cx, Limits::tight(room));
            child.set_position(Point::new(g.rect.x, g.rect.y + strip));
        }
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let origin = cx.bounds().origin();
        let theme = *cx.theme();
        let p = theme.palette();
        let (pressed, moving, pointer, held_bar, hovered_bar) = {
            let st = cx.state::<DockState>();
            (st.pressed.clone(), st.moving, st.pointer, st.bar.clone(), st.hovered_bar.clone())
        };
        for (g, child) in self.groups.iter().zip(&self.children) {
            let r = g.rect.translate(origin);
            let flat = self.style == TabStyle::Flat;
            if !flat {
                cx.scene.paint(r, theme.small_radius(), &theme.paint(Surface::Card));
                cx.scene.paint(Rect::new(r.x, r.y, r.w, self.strip()), theme.small_radius(), &theme.paint(Surface::Inset));
            }
            for (i, (tab, title)) in g.tabs.iter().zip(&g.titles).enumerate() {
                let tab = tab.translate(origin);
                let front = i == g.active;
                match (flat, front) {
                    (false, true) => cx.scene.paint(tab.inset(2.0), theme.small_radius(), &theme.paint(Surface::Raised)),
                    // Marked along its top, and with no line under it: it is one with its panel.
                    (true, true) => cx.scene.fill(Rect::new(tab.x, tab.y, tab.w, 2.0), 0.0, p.accent, None),
                    (true, false) => cx.scene.fill(Rect::new(tab.x, tab.bottom() - 1.0, tab.w, 1.0), 0.0, p.line, None),
                    (false, false) => {}
                }
                if flat {
                    // A line parting it from the tab after it, or from the rest of the strip.
                    cx.scene.fill(Rect::new(tab.right() - 1.0, tab.y, 1.0, tab.h), 0.0, p.line, None);
                }
                let s = title.size();
                cx.scene.push_clip(tab.inset(4.0));
                cx.scene.text(title, Point::new(tab.x + TAB_X.min((tab.w - s.w).max(8.0) / 2.0), tab.y + ((tab.h - s.h) * 0.5).round()), if front { p.text } else { p.muted });
                cx.scene.pop_clip();
            }
            if flat {
                // Under the rest of the strip, past the last tab.
                let past = g.tabs.last().map_or(r.x, |t| t.translate(origin).right());
                cx.scene.fill(Rect::new(past, r.y + self.strip() - 1.0, (r.right() - past).max(0.0), 1.0), 0.0, p.line, None);
            }
            cx.scene.push_clip(Rect::new(r.x, r.y + self.strip(), r.w, (r.h - self.strip()).max(0.0)));
            child.draw(cx);
            cx.scene.pop_clip();
        }
        for bar in &self.bars {
            let active = held_bar.as_ref() == Some(&bar.path) || hovered_bar.as_ref() == Some(&bar.path);
            let r = bar.rect.translate(origin);
            match (self.style, active) {
                (TabStyle::Raised, true) => cx.scene.fill(r.inset(1.0), 2.0, p.accent.with_alpha(0.55), None),
                (TabStyle::Raised, false) => {}
                // Always there as a hairline; thicker and in the accent while it is to hand.
                (TabStyle::Flat, true) => cx.scene.fill(r.inset(-1.0), 0.0, p.accent, None),
                (TabStyle::Flat, false) => cx.scene.fill(r, 0.0, p.line, None),
            }
        }
        // Where a tab being dragged would land, over everything.
        if let (true, Some((panel, _)), Some((i, side))) = (moving, &pressed, self.target(origin, pointer)) {
            cx.scene.push_layer();
            let g = &self.groups[i];
            let same = g.panels.iter().any(|x| x == panel);
            if !(same && (side == Side::Middle || g.panels.len() == 1)) {
                let body = g.rect.translate(origin);
                let (inset, radius) = if self.style == TabStyle::Flat { (0.0, 0.0) } else { (3.0, theme.small_radius()) };
                cx.scene.fill(landing(body, side).inset(inset), radius, p.accent.with_alpha(0.22), Some((2.0, p.accent)));
            }
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let origin = cx.bounds().origin();
        let tab_at = |p: Point| self.groups.iter().find_map(|g| g.tabs.iter().position(|t| t.translate(origin).contains(p)).map(|i| g.panels[i].clone()));
        let reach = if self.style == TabStyle::Flat { -4.0 } else { -2.0 };
        let bar_at = |p: Point| self.bars.iter().find(|b| b.rect.translate(origin).inset(reach).contains(p));
        match event {
            Event::PointerPressed { pos, button: PointerButton::Primary } => {
                if let Some(bar) = bar_at(*pos) {
                    cx.state::<DockState>().bar = Some(bar.path.clone());
                    return Status::Captured;
                }
                if let Some(panel) = tab_at(*pos) {
                    let st = cx.state::<DockState>();
                    (st.pressed, st.moving, st.pointer) = (Some((panel, *pos)), false, *pos);
                    return Status::Captured;
                }
            }
            Event::PointerMoved { pos } => {
                let (held, pressed) = (cx.state::<DockState>().bar.clone(), cx.state::<DockState>().pressed.clone());
                if let Some(bar) = held.as_ref().and_then(|path| self.bars.iter().find(|b| b.path == *path)) {
                    cx.set_cursor(if bar.across { CursorIcon::Resize(ResizeEdge::East) } else { CursorIcon::Resize(ResizeEdge::South) });
                    let along = if bar.across { pos.x - origin.x } else { pos.y - origin.y };
                    let share = (along - bar.span.0 - self.gap() / 2.0) / (bar.span.1 - self.gap()).max(1.0);
                    cx.emit((self.on_change)(self.layout.clone().with_share(&bar.path, share)));
                    return Status::Captured;
                }
                if let Some((_, from)) = pressed {
                    let st = cx.state::<DockState>();
                    st.pointer = *pos;
                    st.moving |= (pos.x - from.x).abs() > SLACK || (pos.y - from.y).abs() > SLACK;
                    cx.request_redraw();
                    return Status::Captured;
                }
                let over = bar_at(*pos).map(|b| (b.path.clone(), b.across));
                if let Some((_, across)) = &over {
                    cx.set_cursor(if *across { CursorIcon::Resize(ResizeEdge::East) } else { CursorIcon::Resize(ResizeEdge::South) });
                } else if tab_at(*pos).is_some() {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                let over = over.map(|(path, _)| path);
                if cx.state::<DockState>().hovered_bar != over {
                    cx.state::<DockState>().hovered_bar = over;
                    cx.request_redraw();
                }
            }
            Event::PointerReleased { pos, button: PointerButton::Primary } => {
                let st = cx.state::<DockState>();
                let (bar, pressed, moving) = (st.bar.take(), st.pressed.take(), std::mem::take(&mut st.moving));
                if bar.is_some() {
                    cx.request_redraw();
                    return Status::Captured;
                }
                if let Some((panel, _)) = pressed {
                    cx.request_redraw();
                    let next = match (moving, self.target(origin, *pos)) {
                        // Dragged somewhere: put against whatever is in front there.
                        (true, Some((i, side))) => {
                            let g = &self.groups[i];
                            self.layout.clone().with(&panel, &g.panels[g.active], side)
                        }
                        // Dragged off everything: left where it was.
                        (true, None) => return Status::Captured,
                        // Only clicked: brought to the front.
                        (false, _) => {
                            let mut d = self.layout.clone();
                            d.show(&panel);
                            d
                        }
                    };
                    if next != self.layout {
                        cx.emit((self.on_change)(next));
                    }
                    return Status::Captured;
                }
            }
            Event::PointerLeft => cx.state::<DockState>().hovered_bar = None,
            _ => {}
        }
        // Whatever the dock has no use for is the panels'.
        Element::event_children(&mut self.children, cx, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Dock {
        Dock::beside(Dock::tabs(["scene", "assets"]), 0.25, Dock::above(Dock::tabs(["view"]), 0.7, Dock::tabs(["console", "log"])))
    }

    #[test]
    fn an_arrangement_is_written_as_a_line_and_read_back() {
        let mut d = editor();
        d.show("log");
        assert_eq!(d.encode(), "row(0.25, tabs(*scene, assets), column(0.7, tabs(*view), tabs(console, *log)))");
        assert_eq!(Dock::parse(&d.encode()), Some(d.clone()));
        assert_eq!((d.panels(), d.shown()), (vec!["scene", "assets", "view", "console", "log"], vec!["scene", "view", "log"]));
        assert_eq!(Dock::parse("  tabs( a ,*b b, c )  "), Some(Dock::Tabs { panels: vec!["a".into(), "b b".into(), "c".into()], active: 1 }));
        // A share out of bounds is brought within them; what is not an arrangement is nothing.
        assert_eq!(Dock::parse("row(7, tabs(a), tabs(b))"), Some(Dock::beside(Dock::tabs(["a"]), 0.92, Dock::tabs(["b"]))));
        for odd in ["", "tabs()", "tabs(a", "row(0.5, tabs(a))", "pile(0.5, tabs(a), tabs(b))", "tabs(a) and more", "row(half, tabs(a), tabs(b))"] {
            assert_eq!(Dock::parse(odd), None, "{odd:?}");
        }
    }

    #[test]
    fn panels_are_closed_and_moved_about() {
        // Closed: its group closes up, and an empty group gives way to its neighbour.
        assert_eq!(editor().without("assets").unwrap().encode(), "row(0.25, tabs(*scene), column(0.7, tabs(*view), tabs(*console, log)))");
        assert_eq!(editor().without("view").unwrap().encode(), "row(0.25, tabs(*scene, assets), tabs(*console, log))");
        assert_eq!(editor().without("scene").unwrap().without("assets").unwrap().encode(), "column(0.7, tabs(*view), tabs(*console, log))");
        assert_eq!(Dock::tabs(["only"]).without("only"), None);
        assert_eq!(editor().without("not there"), Some(editor()));
        let mut front = Dock::Tabs { panels: vec!["a".into(), "b".into(), "c".into()], active: 2 };
        front = front.without("a").unwrap();
        assert_eq!(front.shown(), ["c"], "the one in front stays in front");
        assert_eq!(front.without("c").unwrap().shown(), ["b"]);

        // Moved among another group's tabs, where it comes to the front.
        assert_eq!(editor().with("assets", "console", Side::Middle).encode(), "row(0.25, tabs(*scene), column(0.7, tabs(*view), tabs(console, log, *assets)))");
        // To one side of a group: a group of its own there.
        assert_eq!(editor().with("log", "view", Side::Right).encode(), "row(0.25, tabs(*scene, assets), column(0.7, row(0.5, tabs(*view), tabs(*log)), tabs(*console)))");
        assert_eq!(editor().with("log", "scene", Side::Top).encode(), "row(0.25, column(0.5, tabs(*log), tabs(*scene, assets)), column(0.7, tabs(*view), tabs(*console)))");
        // Out of its own group to the side of it, leaving the rest behind.
        assert_eq!(editor().with("console", "console", Side::Left).encode(), "row(0.25, tabs(*scene, assets), column(0.7, tabs(*view), row(0.5, tabs(*console), tabs(*log))))");
        // The last of a group moved away: the group goes.
        assert_eq!(editor().with("view", "scene", Side::Middle).encode(), "row(0.25, tabs(scene, assets, *view), tabs(*console, log))");
        // One that was not there is added.
        assert_eq!(editor().with("profiler", "log", Side::Middle).shown(), ["scene", "view", "profiler"]);
        // Nowhere to go, or onto itself, or alone beside itself: as it was.
        assert_eq!(editor().with("log", "nowhere", Side::Left), editor());
        assert_eq!(editor().with("log", "log", Side::Middle), editor());
        assert_eq!(editor().with("view", "view", Side::Left), editor());
        // Dropped among its own tabs it only comes forward.
        assert_eq!(editor().with("assets", "scene", Side::Middle).shown(), ["assets", "view", "console"]);
        // A bar dragged gives the split a new share, kept within bounds.
        assert_eq!(editor().with_share(&[], 0.4).encode(), "row(0.4, tabs(*scene, assets), column(0.7, tabs(*view), tabs(*console, log)))");
        assert_eq!(editor().with_share(&[true], 2.0).encode(), "row(0.25, tabs(*scene, assets), column(0.92, tabs(*view), tabs(*console, log)))");
        assert_eq!(editor().with_share(&[false], 0.5), editor(), "that way leads to no split");
    }
}
