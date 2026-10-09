//! Which apps are opened, how often and how lately: what puts an app
//! near the top of the list before anything is typed.
//!
//! Each app has a weight. Opening it adds one. Left alone, the weight
//! halves every week. So the weight is, near enough, how many times the
//! app was opened in the last week or two, with last month counting for
//! little and last year for nothing: an app used every day stands high,
//! one used once yesterday stands above one used five times a month ago,
//! and nothing stays at the top for what it used to be.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How long it takes an app's weight to halve, in seconds: a week.
const HALF_LIFE: f64 = 7.0 * 24.0 * 3600.0;
/// The most that habit can add to how well a name fits what is typed:
/// enough to choose between two that fit about as well, and never enough
/// to beat a name that fits better.
const MOST_NUDGE: f32 = 80.0;
/// A weight under this is forgotten: an app last opened months ago.
const FORGOTTEN: f32 = 0.01;

/// What is kept of one app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Habit {
    /// Its weight as it was at `last`.
    weight: f32,
    /// When it was last opened, in seconds since 1970.
    last: u64,
}

/// The time now, in seconds since 1970.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Habit {
    /// Its weight at `now`: what it was, halved for every week since.
    pub fn weight(&self, now: u64) -> f32 {
        (f64::from(self.weight) * 0.5f64.powf(now.saturating_sub(self.last) as f64 / HALF_LIFE)) as f32
    }
}

/// Every app's habit, by the app's path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Habits(HashMap<PathBuf, Habit>);

impl Habits {
    /// Notes that an app was opened at `now`.
    pub fn opened(&mut self, app: &Path, now: u64) {
        let weight = self.weight(app, now) + 1.0;
        self.0.insert(app.to_path_buf(), Habit { weight, last: now });
    }

    /// An app's weight at `now`; nothing for one never opened.
    pub fn weight(&self, app: &Path, now: u64) -> f32 {
        self.0.get(app).map_or(0.0, |h| h.weight(now))
    }

    /// What an app's habit adds to how well its name fits what is typed.
    /// It grows quickly with the first few openings and slowly after, so
    /// that an app opened a hundred times does not count ten times one
    /// opened ten.
    pub fn nudge(&self, app: &Path, now: u64) -> i64 {
        let weight = self.weight(app, now);
        (MOST_NUDGE * weight / (weight + 3.0)).round() as i64
    }

    /// The habits as lines of `weight`, `last` and path, for keeping.
    /// Those faded to nothing are left out.
    pub fn encode(&self, now: u64) -> String {
        let mut lines: Vec<String> = self.0.iter().filter(|(_, h)| h.weight(now) >= FORGOTTEN).map(|(path, h)| format!("{:.4}\t{}\t{}", h.weight, h.last, path.display())).collect();
        lines.sort();
        lines.join("\n") + "\n"
    }

    /// Reads what [`encode`](Self::encode) wrote. A line of the older
    /// form, a count and a path, is taken as that many openings of late:
    /// counted from `now`, and no more than a handful, since how long ago
    /// they were is not known.
    pub fn decode(text: &str, now: u64) -> Self {
        let mut out = HashMap::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.splitn(3, '\t').collect();
            let (path, habit) = match fields.as_slice() {
                [weight, last, path] => match (weight.parse::<f32>(), last.parse::<u64>()) {
                    (Ok(weight), Ok(last)) => (path, Habit { weight, last }),
                    _ => continue,
                },
                [count, path] => match count.parse::<u32>() {
                    Ok(count) => (path, Habit { weight: count.min(5) as f32, last: now }),
                    Err(_) => continue,
                },
                _ => continue,
            };
            if habit.weight.is_finite() && habit.weight > 0.0 && !path.is_empty() {
                out.insert(PathBuf::from(path), habit);
            }
        }
        Self(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 3600;
    const NOW: u64 = 1_800_000_000;

    #[test]
    fn opening_adds_to_an_apps_weight_and_time_takes_it_away() {
        let mut h = Habits::default();
        let (mail, game) = (Path::new("/Applications/Mail.app"), Path::new("/Applications/Game.app"));
        assert_eq!(h.weight(mail, NOW), 0.0);
        h.opened(mail, NOW);
        assert_eq!(h.weight(mail, NOW), 1.0);
        assert!((h.weight(mail, NOW + 7 * DAY) - 0.5).abs() < 0.001, "half as much a week on");
        assert!((h.weight(mail, NOW + 14 * DAY) - 0.25).abs() < 0.001);
        // Opened again a week on, it is what was left and one more.
        h.opened(mail, NOW + 7 * DAY);
        assert!((h.weight(mail, NOW + 7 * DAY) - 1.5).abs() < 0.001);
        // Used every day for a fortnight, it stands near ten; it does not climb for ever.
        for day in 0..14 {
            h.opened(game, NOW + day * DAY);
        }
        let daily = h.weight(game, NOW + 13 * DAY);
        assert!(daily > 7.0 && daily < 11.0, "{daily}");
        for day in 14..120 {
            h.opened(game, NOW + day * DAY);
        }
        assert!(h.weight(game, NOW + 119 * DAY) < 11.0);
    }

    #[test]
    fn lately_counts_for_more_than_often_long_ago() {
        let mut h = Habits::default();
        let (old, new) = (Path::new("/old.app"), Path::new("/new.app"));
        // Five times a month ago; once yesterday.
        for _ in 0..5 {
            h.opened(old, NOW - 30 * DAY);
        }
        h.opened(new, NOW - DAY);
        assert!(h.weight(new, NOW) > h.weight(old, NOW), "{} against {}", h.weight(new, NOW), h.weight(old, NOW));
        // But five times last week still beats once yesterday.
        let mut h = Habits::default();
        for _ in 0..5 {
            h.opened(old, NOW - 6 * DAY);
        }
        h.opened(new, NOW - DAY);
        assert!(h.weight(old, NOW) > h.weight(new, NOW));
    }

    #[test]
    fn habit_nudges_a_search_and_cannot_run_away_with_it() {
        let mut h = Habits::default();
        let app = Path::new("/a.app");
        assert_eq!(h.nudge(app, NOW), 0);
        h.opened(app, NOW);
        let once = h.nudge(app, NOW);
        for _ in 0..200 {
            h.opened(app, NOW);
        }
        let always = h.nudge(app, NOW);
        assert!(once >= 15 && always > once && always <= MOST_NUDGE as i64, "{once} then {always}");
    }

    #[test]
    fn habits_are_written_down_and_read_back() {
        let mut h = Habits::default();
        h.opened(Path::new("/Applications/My App.app"), NOW);
        h.opened(Path::new("/Applications/My App.app"), NOW + DAY);
        h.opened(Path::new("/Applications/Other.app"), NOW);
        let back = Habits::decode(&h.encode(NOW + DAY), NOW + DAY);
        assert!((back.weight(Path::new("/Applications/My App.app"), NOW + DAY) - h.weight(Path::new("/Applications/My App.app"), NOW + DAY)).abs() < 0.001);
        assert_eq!(back.0.len(), 2);
        // One not opened for a year is not kept.
        assert_eq!(Habits::decode(&h.encode(NOW + 400 * DAY), NOW + 400 * DAY), Habits::default());
        // The older form, a count and a path: taken as a few openings of late.
        let old = Habits::decode("1\t/Applications/Spotify.app\n40\t/Applications/Safari.app\nnonsense\n\t\n", NOW);
        assert_eq!((old.weight(Path::new("/Applications/Spotify.app"), NOW), old.weight(Path::new("/Applications/Safari.app"), NOW), old.0.len()), (1.0, 5.0, 2));
    }
}
