//! Somewhere to let go of what is being dragged: a field a row of a tree
//! is dropped on, a view a file is dropped into.
//!
//! What is carried is a value of the app's own kind, begun by whatever it
//! was dragged from (a [`tree`](super::tree) carries the ID of the row);
//! a drop area takes those of the kind it is for and leaves the rest.

use std::marker::PhantomData;

use armature_render::{Point, Size};

use crate::ThemeCx;
use crate::core::{Cx, DrawCx, Element, EventCx, Length, Limits, Widget};
use crate::event::{Event, Status};

/// A place that takes what is dropped on it: see [`drop_area`].
pub struct DropArea<T, M> {
    child: [Element<M>; 1],
    on_drop: Box<dyn Fn(T, Point) -> M>,
    kind: PhantomData<T>,
}

/// `content`, as somewhere to drop a `T` that is being dragged: while one
/// is carried over it, it is outlined, and let go there, `on_drop` is
/// given it, with where in the area it was let go, from the area's own
/// top left corner. Anything else carried passes it by.
pub fn drop_area<T: 'static, M: 'static>(content: impl Into<Element<M>>, on_drop: impl Fn(T, Point) -> M + 'static) -> DropArea<T, M> {
    DropArea { child: [content.into()], on_drop: Box::new(on_drop), kind: PhantomData }
}

impl<T: 'static, M: 'static> Widget<M> for DropArea<T, M> {
    fn width(&self) -> Length {
        self.child[0].width()
    }

    fn height(&self) -> Length {
        self.child[0].height()
    }

    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.child
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = self.child[0].layout(cx, limits);
        self.child[0].set_position(Point::ZERO);
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        self.child[0].draw(cx);
        // Something of its kind is over it: it would be taken here.
        if cx.dragged::<T>().is_some() && cx.is_hovered() {
            let b = cx.bounds();
            let (accent, radius) = (cx.theme().palette().accent, cx.theme().small_radius());
            cx.scene.push_layer();
            cx.scene.fill(b, radius, accent.with_alpha(0.14), Some((2.0, accent)));
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        match event {
            Event::PointerReleased { pos, .. } if b.contains(*pos) => {
                if let Some(carried) = cx.take_drag::<T>() {
                    cx.emit((self.on_drop)(carried, Point::new(pos.x - b.x, pos.y - b.y)));
                    return Status::Captured;
                }
            }
            // Drawn again as what is carried comes over it and leaves.
            Event::PointerMoved { .. } if cx.dragged::<T>().is_some() => cx.request_redraw(),
            _ => {}
        }
        Element::event_children(&mut self.child, cx, event)
    }
}

impl<T: 'static, M: 'static> From<DropArea<T, M>> for Element<M> {
    fn from(w: DropArea<T, M>) -> Self {
        Element::new(w)
    }
}
