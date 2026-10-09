//! Prose written as Markdown and shown as what it means, with links that
//! can be followed.
//!
//! Needs a GPU adapter.

use neo::prelude::*;
use neo::testing::Harness;
use neo::{Point, Size};

struct Page {
    text: String,
    followed: Vec<String>,
}

impl App for Page {
    type Message = String;

    fn update(&mut self, to: String) {
        self.followed.push(to);
    }

    fn view(&self) -> Element<String> {
        container(markdown(&self.text).on_link(|to| to)).padding(10.0).width(Length::Fill).into()
    }

    fn window(&self) -> neo::WindowSettings {
        neo::WindowSettings { decorations: Decorations::System, ..Default::default() }
    }
}

fn page(text: &str) -> Harness<Page> {
    let mut h = Harness::new(Page { text: text.into(), followed: vec![] }, Size::new(420.0, 420.0)).expect("a GPU adapter is required for these tests");
    h.render(1.0);
    h
}

const TEXT: &str = "# The plan\n\nFirst **read** the file, then *change* `main`.\n\n- one thing\n- another, see [the notes](https://example.com/notes)\n\n```rust\nfn main() {\n    println!(\"goodbye\"); // changed\n}\n```\n\n> And a word of caution.\n\n---\n\nDone.";

#[test]
fn markdown_is_shown_as_what_it_means_and_not_as_its_marks() {
    let mut h = page(TEXT);
    let shown = h.render(1.0);
    // The same words with no marks at all are a different, plainer picture.
    let plain = page("The plan\n\nFirst read the file, then change main.\n\none thing\nanother, see the notes\n\nfn main() {\n    println!(\"goodbye\"); // changed\n}\n\nAnd a word of caution.\n\nDone.").render(1.0);
    assert!(shown != plain);
    // Some of it is in colour: the link, and the code's own colours.
    let coloured = |px: &[u8]| px.chunks(4).filter(|p| (p[0] as i32 - p[2] as i32).abs() > 40).count();
    assert!(coloured(&shown) > 60 && coloured(&plain) < coloured(&shown) / 4, "{} against {}", coloured(&shown), coloured(&plain));
    // Still arriving, with its marks not all closed: shown as far as it goes, without a fuss.
    for cut in [5, 20, 31, 47, 70, 95, 120, 150, TEXT.len() - 3] {
        let part: String = TEXT.chars().take(cut).collect();
        page(&part).render(1.0);
    }
}

#[test]
fn a_link_is_followed_with_a_click_and_the_words_round_it_are_not() {
    let mut h = page(TEXT);
    // Every line of the page, clicked along: only the link's words say anything, and they say where it leads.
    for y in (12..400).step_by(6) {
        for x in (14..400).step_by(12) {
            h.click(Point::new(x as f32, y as f32));
        }
    }
    let followed = &h.app().followed;
    assert!(!followed.is_empty() && followed.iter().all(|to| to == "https://example.com/notes"), "{followed:?}");
    assert!(followed.len() <= 18, "only its own few words, not the line they are on: {}", followed.len());
}

/// A picture of it, to look at by hand:
/// `NEO_MD_PNG=/tmp/md.png cargo test -p neo --test markdown a_picture -- --ignored`.
#[test]
#[ignore]
fn a_picture() {
    let mut h = page(TEXT);
    h.save_png(std::env::var("NEO_MD_PNG").expect("NEO_MD_PNG"), 2.0).unwrap();
}
