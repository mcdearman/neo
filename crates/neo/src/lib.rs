//! # Neo
//!
//! A cross-platform GUI toolkit built for the Neo desktop. Interfaces are
//! described with the Elm architecture: an [`App`] owns its state, turns
//! messages into state changes in `update`, and describes the interface in
//! `view`. Neo keeps widget state such as hover, focus and animation between
//! rebuilds.
//!
//! Every widget paints through the [`Theme`], so the same code renders in
//! light or dark, with any accent, and with translucent **glass** windows.
//!
//! ```no_run
//! use neo::prelude::*;
//!
//! #[derive(Default)]
//! struct Counter { n: i32 }
//!
//! #[derive(Clone)]
//! enum Msg { Inc, Dec }
//!
//! impl App for Counter {
//!     type Message = Msg;
//!     fn update(&mut self, m: Msg) {
//!         match m { Msg::Inc => self.n += 1, Msg::Dec => self.n -= 1 }
//!     }
//!     fn view(&self) -> Element<Msg> {
//!         row()
//!             .spacing(12.0)
//!             .push(button("−").on_press(Msg::Dec))
//!             .push(text(self.n.to_string()).role(TextRole::Heading))
//!             .push(button("+").on_press(Msg::Inc))
//!             .into()
//!     }
//! }
//!
//! fn main() -> Result<(), neo::Error> {
//!     neo::run(Counter::default())
//! }
//! ```

mod anim;
mod app;
mod core;
mod event;
mod platform;
mod runtime;
mod shell;
pub mod testing;
pub mod widgets;

pub use anim::Anim;
pub use app::{App, Decorations, Proxy, Subscription, WindowGeometry, WindowSettings, WindowState};
pub use core::{Align, Cx, CursorIcon, DrawCx, Element, EventCx, Length, Limits, Padding, ResizeEdge, Widget, WidgetId, WindowRequest};
pub use event::{Event, Key, KeyEvent, Modifiers, PointerButton, Status};
pub use runtime::Ui;
pub use shell::{run, Error};

pub use neo_render::{Corners, FontFamily, Image, Point, Rect, Scene, Size, TextLayout, TextStyle};
pub use neo_theme::{self as theme, icons, Accent, Color, Glass, Scheme, Surface, TextRole, Theme, Weight};

/// Everything an application usually needs.
pub mod prelude {
    pub use crate::widgets::*;
    pub use crate::{icons, run, Accent, Align, App, Color, Decorations, Element, Glass, Length, Padding, Proxy, Scheme, Subscription, Surface, TextRole, Theme, Weight, WindowSettings, WindowState};
}
