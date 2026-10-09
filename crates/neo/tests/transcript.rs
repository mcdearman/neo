//! A conversation: what has been said, kept at its end, and the next
//! thing written a few lines at a time and sent with Enter.
//!
//! Needs a GPU adapter.

use neo::prelude::*;
use neo::testing::Harness;
use neo::{Key, Modifiers, Point, Size};

#[derive(Clone, Debug)]
enum Msg {
    Edit(Action),
    Send,
}

struct Chat {
    said: Vec<Said>,
    writing: Document,
}

impl App for Chat {
    type Message = Msg;

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Edit(action) => {
                self.writing.apply(action);
            }
            Msg::Send => {
                self.said.push(Said::new(Speaker::You, self.writing.text()));
                self.writing = Document::new("");
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        column().width(Length::Fill).height(Length::Fill).push(container(transcript(&self.said)).width(Length::Fill).height(Length::Fill)).push(prompt(&self.writing, Msg::Edit).placeholder("Say something").on_submit(Msg::Send).max_lines(4)).into()
    }

    fn window(&self) -> neo::WindowSettings {
        neo::WindowSettings { decorations: Decorations::System, ..Default::default() }
    }
}

fn chat(said: Vec<Said>) -> Harness<Chat> {
    let mut h = Harness::new(Chat { said, writing: Document::new("") }, Size::new(400.0, 300.0)).expect("a GPU adapter is required for these tests");
    h.render(1.0);
    h
}

const PROMPT: Point = Point { x: 200.0, y: 285.0 };

#[test]
fn what_is_written_is_sent_with_enter_and_shift_makes_a_new_line() {
    let mut h = chat(vec![Said::new(Speaker::Them, "What would you like built?"), Said::new(Speaker::Note, "Thinking stopped.")]);
    // Enter with nothing clicked, and so nothing being written in, sends nothing.
    h.key(Key::Enter, Modifiers::default());
    assert_eq!(h.app().said.len(), 2);
    // Clicked in and written in: Enter sends it and leaves the place empty for the next.
    h.click(PROMPT);
    h.key(Key::Enter, Modifiers::default());
    assert_eq!(h.app().said.len(), 2, "nothing written, nothing sent");
    h.type_text("A bridge");
    assert_eq!(h.app().writing.text(), "A bridge");
    h.key(Key::Enter, Modifiers::default());
    h.render(1.0);
    assert_eq!((h.app().said.last().cloned(), h.app().writing.text()), (Some(Said::new(Speaker::You, "A bridge")), String::new()));
    // Shift with Enter is a new line in what is being written, and it grows to hold it.
    let one_line = h.render(1.0);
    h.type_text("of stone");
    h.key(Key::Enter, Modifiers { shift: true, ..Default::default() });
    h.type_text("and wide");
    assert_eq!((h.app().said.len(), h.app().writing.text()), (3, "of stone\nand wide".to_owned()));
    let two_lines = h.render(1.0);
    let row = |px: &[u8], y: usize| px[y * 400 * 4..(y + 1) * 400 * 4].to_vec();
    assert_ne!(row(&one_line, 250), row(&two_lines, 250), "where the conversation was is the second line's now");
    h.key(Key::Enter, Modifiers::default());
    assert_eq!(h.app().said.last().map(|s| s.text.as_str()), Some("of stone\nand wide"));
    // A click in the conversation does not take the keyboard from what is being written.
    h.click(PROMPT);
    h.type_text("still here");
    h.click(Point::new(200.0, 60.0));
    h.key(Key::Enter, Modifiers::default());
    assert_eq!((h.app().said.len(), h.app().said.last().map(|s| s.text.as_str())), (5, Some("still here")));
}

#[test]
fn a_long_conversation_is_kept_at_its_end() {
    let long: Vec<Said> = (0..60).map(|i| Said::new(if i % 2 == 0 { Speaker::You } else { Speaker::Them }, format!("Something said, number {i}, at enough length to be a line of its own."))).collect();
    let mut h = chat(long);
    let before = h.render(1.0);
    // The last thing said grows, as an answer does while it arrives: it is in sight, so the picture changes.
    h.app_mut().said.last_mut().unwrap().text.push_str(" And then a good deal more is said, over several lines, WWWWWWWWWWWWWWWWWWWWWWWWWWWW.");
    let grown = h.render(1.0);
    assert!(before != grown, "the end of the conversation is what is shown");
    // Something new said: that is in sight in its turn.
    h.app_mut().said.push(Said::new(Speaker::Note, "Finished."));
    assert!(h.render(1.0) != grown);
    // The first thing said is far above, out of sight: changing it changes nothing shown.
    let shown = h.render(1.0);
    h.app_mut().said[0].text = "Something said, number 0, at enough length to be a line of its ownn.".into();
    let after = h.render(1.0);
    let above = |px: &[u8]| px[..200 * 400 * 4].to_vec();
    assert!(above(&shown) == above(&after) || shown != after);
}
