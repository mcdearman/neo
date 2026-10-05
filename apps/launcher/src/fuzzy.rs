//! Fuzzy matching of what was typed against app names.
//!
//! Every name gets a score, so there is always a best answer. Names that
//! contain the typed letters in order rank first, with the tightest and
//! most natural matches on top; names that do not are ranked by how much
//! of the query they share, below all of those.

/// How well a name matches. Higher is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Score(pub i32);

/// Scores below this are loose resemblances, not real matches.
pub const LOOSE: i32 = -1_000_000;

fn is_boundary(prev: char, cur: char) -> bool {
    // The start of a word, or a capital inside camelCase.
    !prev.is_alphanumeric() || (prev.is_lowercase() && cur.is_uppercase())
}

/// The best score for matching `query`, letter by letter in order, somewhere
/// in `name`. `None` if the letters do not all appear in order.
fn in_order(query: &[char], name: &[char], lower: &[char]) -> Option<i32> {
    const MATCH: i32 = 16;
    const WORD_START: i32 = 26;
    const CONSECUTIVE: i32 = 18;
    const GAP: i32 = 2;
    // best[j]: the best score with the query so far ending at name[j].
    let mut best: Vec<Option<i32>> = vec![None; name.len()];
    for (qi, q) in query.iter().enumerate() {
        let mut next: Vec<Option<i32>> = vec![None; name.len()];
        // The best score of any earlier position, less the gap to here.
        let mut carried: Option<i32> = None;
        for j in 0..name.len() {
            if lower[j] == *q {
                let bonus = if j == 0 || is_boundary(name[j - 1], name[j]) { WORD_START } else { 0 };
                let start = if qi == 0 {
                    // Letters skipped before the first match cost a little.
                    Some(MATCH + bonus - (j as i32).min(12))
                } else {
                    let after_gap = carried.map(|s| s + MATCH + bonus);
                    let adjacent = if j > 0 { best[j - 1].map(|s| s + MATCH + bonus + CONSECUTIVE) } else { None };
                    after_gap.max(adjacent)
                };
                next[j] = start;
            }
            if qi > 0 && j > 0 {
                // Carry forward the best earlier end, paying for each skipped letter.
                carried = carried.map(|s| s - GAP).max(best[j - 1].map(|s| s - GAP));
            }
        }
        best = next;
    }
    best.into_iter().flatten().max()
}

/// How many of the query's letters appear in order in the name, skipping
/// query letters that do not fit: the longest common subsequence.
fn shared(query: &[char], lower: &[char]) -> i32 {
    let mut row = vec![0i32; lower.len() + 1];
    for q in query {
        let mut diagonal = 0;
        for j in 0..lower.len() {
            let above = row[j + 1];
            row[j + 1] = if lower[j] == *q { diagonal + 1 } else { above.max(row[j]) };
            diagonal = above;
        }
    }
    row[lower.len()]
}

/// Scores `name` against `query`. Spaces in the query are ignored, so
/// "vs code" and "vscode" match alike.
pub fn score(query: &str, name: &str) -> Score {
    let query: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if query.is_empty() {
        return Score(0);
    }
    let chars: Vec<char> = name.chars().collect();
    let lower: Vec<char> = chars.iter().flat_map(|c| c.to_lowercase().next()).collect();
    if lower.len() != chars.len() {
        // A rare letter whose lower case is longer; match the plain name.
        return Score(LOOSE);
    }
    match in_order(&query, &chars, &lower) {
        Some(s) => {
            let compact: String = lower.iter().filter(|c| !c.is_whitespace()).collect();
            let typed: String = query.iter().collect();
            // Typing the name itself, or its start, beats everything else.
            let exact = if compact == typed {
                400
            } else if compact.starts_with(&typed) {
                200
            } else if compact.contains(&typed) {
                60
            } else {
                0
            };
            // Among equals, the shorter name is the closer match.
            Score(s + exact - (chars.len() as i32).min(40))
        }
        // Not a real match: rank by resemblance, below every real one.
        None => Score(LOOSE + shared(&query, &lower) * 100 - (chars.len() as i32).min(60)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best<'a>(query: &str, names: &[&'a str]) -> &'a str {
        names.iter().max_by_key(|n| score(query, n)).copied().unwrap()
    }

    const APPS: &[&str] = &["Safari", "System Settings", "Visual Studio Code", "Finder", "Firefox", "Final Cut Pro", "Calculator", "Calendar", "Neo Files", "Notes", "Photo Booth", "Photos", "Preview", "Terminal", "TextEdit", "Activity Monitor", "App Store", "Spotify", "Discord"];

    #[test]
    fn finds_what_people_type() {
        for (query, want) in [
            ("saf", "Safari"),
            ("vsc", "Visual Studio Code"),
            ("vs code", "Visual Studio Code"),
            ("code", "Visual Studio Code"),
            ("ff", "Firefox"),
            ("fire", "Firefox"),
            ("calc", "Calculator"),
            ("cal", "Calendar"),
            ("term", "Terminal"),
            ("photos", "Photos"),
            ("pb", "Photo Booth"),
            ("am", "Activity Monitor"),
            ("settings", "System Settings"),
            ("fcp", "Final Cut Pro"),
            ("TEXT", "TextEdit"),
            ("neo", "Neo Files"),
        ] {
            assert_eq!(best(query, APPS), want, "typing {query:?}");
        }
    }

    #[test]
    fn there_is_always_a_closest_answer() {
        // Misspelt, or letters out of order: no name contains these in order.
        assert!(score("safarii", "Safari").0 < 0 && score("safarii", "Safari") > score("safarii", "Discord"));
        assert_eq!(best("safarii", APPS), "Safari");
        assert_eq!(best("fierfox", APPS), "Firefox");
        assert_eq!(best("spotfy x", APPS), "Spotify");
        // And a real match always outranks a resemblance.
        assert!(score("no", "Notes") > score("notez", "Notes"));
        assert!(score("zzz", "Safari").0 <= LOOSE, "nothing in common is the weakest answer, but still an answer");
    }

    #[test]
    fn tighter_matches_rank_higher() {
        // The same letters, together at a word start, beat scattered ones.
        assert!(score("not", "Notes") > score("not", "Neo Terminal"));
        assert!(score("pho", "Photos") > score("pho", "Photo Booth"), "the shorter name wins a tie");
        assert_eq!(score("", "Anything"), Score(0));
    }
}
