//! Says which display is which: a large number shown in the middle of a
//! display for a moment, as the system's own display settings do.
//!
//! NeoCap has one window, and a number is wanted on a display it may not
//! be on, on two at once when both are chosen. So each number is a copy
//! of NeoCap started for the purpose (`--identify N --place X,Y,W,H`),
//! which shows it, takes neither the keyboard nor a place in the
//! taskbar, and ends.

use std::time::Duration;

use neo::prelude::*;
use neo::{Point, Rect, Size};

/// How long the number is shown.
const SHOWN: Duration = Duration::from_millis(1800);
/// How large the card with the number on it is.
const CARD: Size = Size::new(260.0, 240.0);

pub struct Identify {
    /// The display's number, counted from 1.
    number: u32,
    /// Where the display is, in points.
    place: Rect,
    done: bool,
}

impl Identify {
    pub fn new(number: u32, place: Rect) -> Self {
        Self { number, place, done: false }
    }
}

/// Reads `X,Y,W,H`, a display's place in points.
pub fn place(said: &str) -> Option<Rect> {
    let numbers: Vec<f32> = said.split(',').map(|n| n.trim().parse().ok()).collect::<Option<_>>()?;
    (numbers.len() == 4 && numbers[2] > 0.0 && numbers[3] > 0.0).then(|| Rect::new(numbers[0], numbers[1], numbers[2], numbers[3]))
}

/// Shows each of `displays` its number: for each, the number from 1, and
/// where it is in points. Returns at once; the numbers go by themselves.
pub fn show(displays: &[(u32, Rect)]) {
    let Ok(program) = std::env::current_exe() else { return };
    for (number, at) in displays {
        let _ = neo_desktop::command(&program).arg("--identify").arg(number.to_string()).arg("--place").arg(format!("{},{},{},{}", at.x, at.y, at.w, at.h)).stdin(std::process::Stdio::null()).spawn();
    }
}

impl App for Identify {
    type Message = ();

    fn title(&self) -> String {
        format!("Display {}", self.number)
    }

    fn update(&mut self, (): ()) {
        self.done = true;
    }

    fn view(&self) -> Element<()> {
        let card = column().spacing(2.0).align(Align::Center).push(text(self.number.to_string()).size(120.0).role(TextRole::Title)).push(text(format!("Display {}", self.number)).tone(Tone::Muted));
        container(card).surface(Surface::Raised).radius(28.0).width(Length::Fill).height(Length::Fill).center().into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: CARD, min_size: None, resizable: false, app_id: Some("org.neo.Recorder".into()), skip_taskbar: true, ..Default::default() }
    }

    fn window_state(&self) -> WindowState {
        // In the middle of its display, over everything, taking nothing.
        let at = Point::new(self.place.x + (self.place.w - CARD.w) / 2.0, self.place.y + (self.place.h - CARD.h) / 2.0);
        WindowState { visible: true, always_on_top: true, bare: true, size: Some(CARD), position: Some(at), hidden_from_capture: false, passive: true }
    }

    fn subscriptions(&self) -> Vec<Subscription<()>> {
        vec![Subscription::every(SHOWN, ())]
    }

    fn should_exit(&self) -> bool {
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_displays_place_is_read_and_its_number_is_put_in_the_middle() {
        assert_eq!(place("1920,0,1920,1080"), Some(Rect::new(1920.0, 0.0, 1920.0, 1080.0)));
        assert!(place("1,2,3").is_none() && place("a,b,c,d").is_none() && place("0,0,0,10").is_none());
        let mut shown = Identify::new(2, Rect::new(1920.0, 0.0, 1920.0, 1080.0));
        let state = shown.window_state();
        assert_eq!(state.position, Some(Point::new(1920.0 + (1920.0 - CARD.w) / 2.0, (1080.0 - CARD.h) / 2.0)));
        assert!(state.passive && state.always_on_top && state.bare && shown.window().skip_taskbar, "over everything, and in nobody's way");
        assert!(!shown.should_exit());
        shown.update(());
        assert!(shown.should_exit(), "and gone after its moment");
    }
}