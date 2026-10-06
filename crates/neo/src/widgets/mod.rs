//! Built-in widgets. Most have a lower-case shorthand constructor.

mod button;
mod container;
mod data;
pub(crate) mod frame;
mod highlight;
mod icon;
mod menu;
mod scrollable;
mod segmented;
mod slider;
mod divider;
pub mod style;
mod text;
mod text_editor;
mod text_input;
mod toggle;

pub use button::{button, icon_button, Button, ButtonKind};
pub use container::{container, Background, Container};
pub use armature::document::{Action, BlockSelection, ClipboardNeed, Document, ExtraSelection, Keymap, Mode as VimMode, ModeStatus, Motion, Pos, Scroll, VimRequest, VimStatus, VimView, INDENT};
pub use highlight::{Kind as SyntaxKind, Language};
pub use data::{gauge, progress_bar, sparkline, Gauge, ProgressBar, Sparkline};
pub use armature::widgets::{column, fit_rect, label, mouse_area, picture, row, stack, Column, Fit, Flex, Justify, Label, Map, MouseArea, Picture, Row, Space, Stack};
pub use icon::{icon, Icon};
pub use menu::{popup_menu, MenuItem, PopupMenu};
pub use scrollable::{scrollable, Scrollable};
pub use segmented::{segmented, Segmented};
pub use slider::{slider, Slider};
pub use divider::Divider;
pub use style::Tone;
pub use text::{text, Text};
pub use text_editor::{text_editor, EditorMark, EditorPopup, EditorToken, PopupKey, TextEditor};
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

into_element!(Text, Icon, Divider, ProgressBar, Gauge, Sparkline);
into_element_generic!(PopupMenu, Button, Container, Scrollable, Segmented, Slider, TextEditor, TextInput, Toggle, Checkbox);
