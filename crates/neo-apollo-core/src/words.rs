//! The words that say what a piece of text is about.
//!
//! A picture's words come from the model that describes it. A passage of
//! a document is too many to ask a model about one by one, so its words
//! are picked here: the ones it uses most that are not the ones every
//! text uses.

/// Words too common to say anything about a text.
const COMMON: &[&str] = &[
    "about", "above", "after", "again", "all", "also", "and", "any", "are", "because", "been", "before", "being", "below", "between", "both", "but", "can", "could", "did", "does", "doing", "down", "during", "each", "few", "for", "from", "further", "had", "has", "have", "having", "her", "here", "hers", "him", "his", "how", "into", "its", "just", "may", "more", "most", "much", "must", "not", "now",
    "off", "once", "one", "only", "other", "our", "ours", "out", "over", "own", "same", "shall", "she", "should", "some", "such", "than", "that", "the", "their", "theirs", "them", "then", "there", "these", "they", "this", "those", "through", "too", "two", "under", "until", "upon", "use", "used", "using", "very", "was", "were", "what", "when", "where", "which", "while", "who", "whom", "why",
    "will", "with", "within", "without", "would", "you", "your", "yours", "get", "got", "like", "make", "made", "many", "new", "see", "way", "well", "yes", "per", "via", "etc", "http", "https", "www", "com",
];

/// A word as the cloud keeps it: lower case, no more than three words
/// long, letters and digits with spaces or hyphens between. `None` if
/// nothing is left that says anything.
pub fn clean(word: &str) -> Option<String> {
    let lower = word.trim().to_lowercase();
    let parts: Vec<&str> = lower.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '\'')).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let joined = parts.join(" ");
    let says_something = joined.chars().filter(|c| c.is_alphabetic()).count() >= 3 && !(parts.len() == 1 && COMMON.contains(&parts[0]));
    says_something.then_some(joined)
}

/// Cleans a list of words, dropping repeats and keeping the order.
pub fn clean_all(words: impl IntoIterator<Item = impl AsRef<str>>, most: usize) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for word in words {
        if let Some(w) = clean(word.as_ref())
            && !out.contains(&w)
            && out.len() < most
        {
            out.push(w);
        }
    }
    out
}

/// The `most` words a text uses most that say something about it, most
/// used first, and of those used equally, the first to appear.
pub fn keywords(text: &str, most: usize) -> Vec<String> {
    let mut seen: Vec<(String, u32)> = vec![];
    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '\'')) {
        let word = word.trim_matches('\'').to_lowercase();
        let word = word.strip_suffix("'s").unwrap_or(&word);
        if word.chars().count() < 4 || !word.chars().all(char::is_alphabetic) || COMMON.contains(&word) {
            continue;
        }
        match seen.iter_mut().find(|(w, _)| w == word) {
            Some((_, n)) => *n += 1,
            None => seen.push((word.to_owned(), 1)),
        }
    }
    // A stable sort, so equals stay in the order they appeared.
    seen.sort_by_key(|s| std::cmp::Reverse(s.1));
    seen.into_iter().take(most).map(|(w, _)| w).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_tidied_and_empty_ones_dropped() {
        assert_eq!(clean("  Golden Retriever "), Some("golden retriever".into()));
        assert_eq!(clean("Sunset!"), Some("sunset".into()));
        assert_eq!(clean("black-and-white"), Some("black-and-white".into()));
        assert_eq!((clean("the"), clean(""), clean("42"), clean("a very long phrase indeed"), clean("ok")), (None, None, None, None, None));
        assert_eq!(clean_all(["Dog", "dog ", "The", "beach", "sand", "sea"], 3), ["dog", "beach", "sand"]);
    }

    #[test]
    fn a_passage_is_about_the_words_it_leans_on() {
        let text = "The invoice is due in March. Payment of the invoice should be made to the supplier; the supplier's bank is below. Invoice number 4411.";
        assert_eq!(keywords(text, 3), ["invoice", "supplier", "march"]);
        assert_eq!(keywords("and the of to", 5), Vec::<String>::new());
        assert_eq!(keywords("", 5), Vec::<String>::new());
    }
}
