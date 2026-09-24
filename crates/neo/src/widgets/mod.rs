//! Built-in widgets. Most have a lower-case shorthand constructor.

mod button;
mod container;
mod data;
mod flex;
pub(crate) mod frame;
mod icon;
pub(crate) mod map;
mod scrollable;
mod segmented;
mod slider;
mod space;
mod stack;
pub mod style;
mod text;
mod text_input;
mod toggle;

pub use button::{button, icon_button, Button, ButtonKind};
pub use container::{container, Background, Container};
pub use data::{gauge, progress_bar, sparkline, Gauge, ProgressBar, Sparkline};
pub use flex::{column, row, Column, Flex, Justify, Row};
pub use icon::{icon, Icon};
pub use map::Map;
pub use scrollable::{scrollable, Scrollable};
pub use segmented::{segmented, Segmented};
pub use slider::{slider, Slider};
pub use space::{Divider, Space};
pub use stack::{stack, Stack};
pub use style::Tone;
pub use text::{text, Text};
pub use text_input::{text_input, TextInput};
pub use toggle::{checkbox, toggle, Checkbox, Toggle};

use crate::core::Element;

macro_rules! into_element {
    ($($t:ident),* $(,)?) => {$(
        impl<M: 'static> From<$t> for Element<M> {
            fn from(w: $t) -> Self {
                Element::new(w)
            }
        }
    )*};
}

macro_rules! into_element_generic {
    ($($t:ident),* $(,)?) => {$(
        impl<M: 'static> From<$t<M>> for Element<M> where $t<M>: crate::core::Widget<M> {
            fn from(w: $t<M>) -> Self {
                Element::new(w)
            }
        }
    )*};
}

into_element!(Text, Icon, Space, Divider, ProgressBar, Gauge, Sparkline);
into_element_generic!(Button, Container, Flex, Scrollable, Segmented, Slider, Stack, TextInput, Toggle, Checkbox);
