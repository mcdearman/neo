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

/// An agent at work: it says something, uses tools, and asks leave.
mod agent {
    use neo::prelude::*;
    use neo::testing::Harness;
    use neo::{Event, Point, Size};

    #[derive(Clone, Debug, PartialEq)]
    pub enum Msg {
        Toggled(u32, bool),
        Allow,
        Always,
        Refuse,
    }

    pub struct Agent {
        pub entries: Vec<Entry<u32, Msg>>,
        pub heard: Vec<Msg>,
    }

    impl App for Agent {
        type Message = Msg;

        fn update(&mut self, m: Msg) {
            self.heard.push(m.clone());
            match m {
                Msg::Toggled(id, open) => {
                    for e in &mut self.entries {
                        if let Entry::Tool(t) = e
                            && t.id == id
                        {
                            t.open = open;
                        }
                    }
                }
                // Answered, what was asked becomes a note of the answer.
                answer => {
                    for e in &mut self.entries {
                        if matches!(e, Entry::Ask(_)) {
                            *e = Entry::Said(Said::new(Speaker::Note, format!("{answer:?}")));
                        }
                    }
                }
            }
        }

        fn view(&self) -> Element<Msg> {
            conversation(&self.entries, Msg::Toggled)
        }

        fn window(&self) -> neo::WindowSettings {
            neo::WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
    }

    pub fn working() -> Harness<Agent> {
        let mut read = ToolRow::new(7, "read_file", "src/main.rs");
        (read.state, read.input, read.result) = (ToolState::Done, "{ \"path\": \"src/main.rs\" }".into(), "fn main() {\n    println!(\"hello\");\n}".into());
        let entries = vec![
            Entry::Said(Said::new(Speaker::You, "Make it say goodbye.")),
            Entry::Said(Said::new(Speaker::Them, "I'll look at the file first.")),
            Entry::Tool(read),
            Entry::Tool(ToolRow::new(8, "run", "cargo build")),
            Entry::Ask(Asking { what: "Run this command?".into(), detail: "cargo build --release".into(), choices: vec![("Allow once".into(), Msg::Allow), ("Always allow".into(), Msg::Always), ("Refuse".into(), Msg::Refuse)] }),
        ];
        let mut h = Harness::new(Agent { entries, heard: vec![] }, Size::new(420.0, 640.0)).expect("a GPU adapter is required for these tests");
        h.render(1.0);
        h
    }

    /// Clicks down the window from `from` until something is heard, and says where that was.
    pub fn click_until_heard(h: &mut Harness<Agent>, x: f32, from: f32) -> Option<f32> {
        let before = h.app().heard.len();
        (0..120).map(|i| from + i as f32 * 3.0).find(|y| {
            h.click(Point::new(x, *y));
            h.render(1.0);
            h.app().heard.len() > before
        })
    }

    pub fn wheel(h: &mut Harness<Agent>, by: f32) {
        h.event(Event::Wheel { pos: Point::new(200.0, 200.0), delta: Point::new(0.0, by) });
        h.render(1.0);
    }
}

#[test]
fn a_tools_row_opens_to_show_what_went_in_and_what_came_out() {
    use agent::*;
    let mut h = working();
    let shut = h.render(1.0);
    // The first row that hears a click is the first tool's: it is to be opened.
    let at = click_until_heard(&mut h, 200.0, 20.0).expect("a tool's row");
    assert_eq!(h.app().heard, [Msg::Toggled(7, true)]);
    let open = h.render(1.0);
    assert!(open != shut, "what it was given and what came back are shown");
    // Clicked again where its line is, it shuts, and the picture is as it was.
    h.click(neo::Point::new(200.0, at));
    assert_eq!(h.app().heard.last(), Some(&Msg::Toggled(7, false)));
    assert!(h.render(1.0) == shut);
    // The one below it is still running, and opens to say there is nothing yet.
    let below = click_until_heard(&mut h, 200.0, at + 24.0).expect("the second tool's row");
    assert!(below > at && h.app().heard.last() == Some(&Msg::Toggled(8, true)));
}

#[test]
fn what_is_asked_is_answered_with_a_button_and_becomes_a_note() {
    use agent::*;
    let mut h = working();
    // Past the tools' rows, the first thing that answers a click at the left is the first answer.
    let mut heard = vec![];
    for y in (150..620).step_by(4) {
        h.click(neo::Point::new(50.0, y as f32));
        h.render(1.0);
        if let Some(m) = h.app().heard.last().filter(|m| !matches!(m, Msg::Toggled(..))) {
            heard.push(m.clone());
            break;
        }
    }
    assert_eq!(heard, [Msg::Allow], "the first button is the usual answer");
    assert!(h.app().entries.iter().all(|e| !matches!(e, Entry::Ask(_))), "and nothing is being asked any more");
    assert!(matches!(h.app().entries.last(), Some(Entry::Said(s)) if s.who == Speaker::Note && s.text == "Allow"));
}

#[test]
fn a_conversation_scrolled_back_stays_where_it_was_put() {
    use agent::*;
    let mut h = working();
    let lines = |n: usize| (0..n).map(|i| format!("Line {i} of a long answer.")).collect::<Vec<_>>().join("\n");
    h.app_mut().entries.push(Entry::Said(Said::new(Speaker::Them, lines(60))));
    let at_end = h.render(1.0);
    // More arrives: at the end, it follows.
    if let Some(Entry::Said(s)) = h.app_mut().entries.last_mut() {
        s.text.push_str("\nAnd a last line, WWWWWWWWWWWWWWWW.");
    }
    let followed = h.render(1.0);
    assert!(followed != at_end);
    // Scrolled back to read something earlier: more arriving moves nothing.
    // (Whichever way the wheel turns for that here.)
    wheel(&mut h, 600.0);
    let back = if h.render(1.0) == followed { -1.0 } else { 1.0 };
    wheel(&mut h, 600.0 * back);
    let reading = h.render(1.0);
    assert!(reading != followed);
    if let Some(Entry::Said(s)) = h.app_mut().entries.last_mut() {
        s.text.push_str("\nStill more, MMMMMMMMMMMMMMMM.");
    }
    h.app_mut().entries.push(Entry::Said(Said::new(Speaker::Note, "Done.")));
    // All but the bar at the right, which is shorter now that there is more to scroll.
    let words = |px: &[u8]| px.chunks(420 * 4).flat_map(|row| row[..400 * 4].to_vec()).collect::<Vec<u8>>();
    assert!(words(&h.render(1.0)) == words(&reading), "what is being read stays put");
    // Scrolled to the end again, it follows again.
    wheel(&mut h, -100_000.0 * back);
    let back = h.render(1.0);
    h.app_mut().entries.push(Entry::Said(Said::new(Speaker::Note, "Really done.")));
    assert!(h.render(1.0) != back);
}

#[test]
fn what_is_written_wraps_to_the_width_and_is_edited_as_prose() {
    let mut h = chat(vec![]);
    h.click(PROMPT);
    let row = |px: &[u8], y: usize| px[y * 400 * 4..(y + 1) * 400 * 4].to_vec();
    let one = h.render(1.0);
    // A sentence longer than the window is across: it goes on to a second row, and the place grows for it.
    h.type_text("A sentence that goes on for rather longer than the window is wide, so that it has to turn.");
    let two = h.render(1.0);
    assert_eq!(h.app().writing.line_count(), 1, "one line as written");
    assert_ne!(row(&one, 252), row(&two, 252), "on two rows as shown");
    // Up goes to the row above within the same line, and what is typed goes in there.
    h.key(Key::Up, Modifiers::default());
    h.type_text("^");
    let at = h.app().writing.text().find('^').unwrap();
    assert!(at > 20 && at < 70, "part of the way along the first row, above where the caret was: {at}");
    // Up again, from the first row: the very start. Down from the last: the very end.
    h.key(Key::Up, Modifiers::default());
    h.key(Key::Up, Modifiers::default());
    h.type_text("<");
    h.key(Key::Down, Modifiers::default());
    h.key(Key::Down, Modifiers::default());
    h.key(Key::Down, Modifiers::default());
    h.type_text(">");
    let text = h.app().writing.text();
    assert!(text.starts_with("<A sentence") && text.ends_with("turn.>"), "{text}");
    // Everything selected and typed over; then a click puts the caret where it lands.
    let all = if cfg!(target_os = "macos") { Modifiers { logo: true, ..Default::default() } } else { Modifiers { ctrl: true, ..Default::default() } };
    h.key(Key::Character("a".into()), all);
    h.type_text("one two three");
    assert_eq!(h.app().writing.text(), "one two three");
    h.click(Point::new(14.0, 285.0));
    h.type_text("zero ");
    assert_eq!(h.app().writing.text(), "zero one two three");
    // Many lines: it grows to the most it may and then scrolls, the caret still in sight.
    h.key(Key::End, Modifiers::default());
    for _ in 0..9 {
        h.key(Key::Enter, Modifiers { shift: true, ..Default::default() });
        h.type_text("more");
    }
    let tall = h.render(1.0);
    assert_eq!(h.app().writing.line_count(), 10);
    assert_ne!(row(&tall, 215), row(&two, 215), "taller than two rows");
    assert_eq!(row(&tall, 150), row(&one, 150), "but no taller than four");
    h.key(Key::Enter, Modifiers::default());
    assert!(h.app().said.last().is_some_and(|s| s.text.starts_with("zero one two three\nmore") && s.text.lines().count() == 10));
}
