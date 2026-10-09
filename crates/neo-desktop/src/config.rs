//! Neo's settings files, which are TOML.
//!
//! What applies to the whole desktop is in `neo.toml` in the settings
//! folder, a table to each kind of thing (`[appearance]`, `[windows]`,
//! `[startup]`); what is one app's own is in `apps/<app>.toml`.
//!
//! Whatever Settings or an app can change is in one of these files, and
//! whatever is in one can be changed by hand: apps look for a change
//! about once a second and take it up. A file is the user's as much as
//! Neo's, so a setting that changes is changed in place: comments, the
//! order of things and anything Neo does not know are left as they were.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::config_dir;

/// The file of settings for the whole desktop.
pub fn desktop_path() -> PathBuf {
    config_dir().join("neo.toml")
}

/// The file of settings for the app called `app`.
pub fn app_path(app: &str) -> PathBuf {
    config_dir().join("apps").join(format!("{app}.toml"))
}

/// When a file was last written, for noticing that it has been.
pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// A settings file, read into memory.
///
/// Settings are found by the way to them: `["appearance", "glass",
/// "opacity"]` is `opacity` in `[appearance.glass]`.
#[derive(Clone, Debug)]
pub struct File {
    path: PathBuf,
    doc: DocumentMut,
    /// What is wrong with the file as it stands, if it could not be read
    /// as TOML. Such a file is not written over.
    problem: Option<String>,
    /// There was no file: it is given a line or two at the top when written.
    fresh: bool,
}

impl File {
    /// Reads the file at `path`. One that is not there is empty.
    pub fn open(path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            Ok(text) => Self { path, fresh: false, ..Self::parse(&text) },
            Err(_) => Self { path, doc: DocumentMut::new(), problem: None, fresh: true },
        }
    }

    /// The settings of the whole desktop.
    pub fn desktop() -> Self {
        Self::open(desktop_path())
    }

    /// The settings of one app.
    pub fn app(app: &str) -> Self {
        Self::open(app_path(app))
    }

    /// A file's text, read. With a mistake in it, it is empty and says so.
    pub fn parse(text: &str) -> Self {
        match text.parse::<DocumentMut>() {
            Ok(doc) => Self { path: PathBuf::new(), doc, problem: None, fresh: false },
            Err(e) => {
                // The first line of what the parser says is the place and the fault.
                let said = e.to_string();
                let line = said.lines().next().unwrap_or("it is not TOML").trim().trim_start_matches("TOML parse error ").to_owned();
                Self { path: PathBuf::new(), doc: DocumentMut::new(), problem: Some(line), fresh: false }
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What is wrong with the file, said for the user: `neo.toml has a
    /// mistake at line 3, column 9`.
    pub fn problem(&self) -> Option<String> {
        let name = self.path.file_name().map_or_else(|| "The settings file".to_owned(), |n| n.to_string_lossy().into_owned());
        self.problem.as_ref().map(|p| format!("{name} has a mistake {p}. Nothing is read from it, or written to it, until that is put right."))
    }

    fn item(&self, at: &[&str]) -> Option<&Item> {
        let (last, tables) = at.split_last()?;
        let mut table = self.doc.as_table();
        for name in tables {
            table = table.get(name)?.as_table()?;
        }
        table.get(last)
    }

    /// Whether there is anything at all there: a setting, or a table of them.
    pub fn has(&self, at: &[&str]) -> bool {
        self.item(at).is_some()
    }

    pub fn text(&self, at: &[&str]) -> Option<&str> {
        self.item(at)?.as_str()
    }

    /// A number, written whole or with a point.
    pub fn number(&self, at: &[&str]) -> Option<f64> {
        let item = self.item(at)?;
        item.as_float().or_else(|| item.as_integer().map(|n| n as f64)).filter(|n| n.is_finite())
    }

    pub fn flag(&self, at: &[&str]) -> Option<bool> {
        self.item(at)?.as_bool()
    }

    /// A list of strings. Anything in it that is not a string is left out.
    pub fn list(&self, at: &[&str]) -> Option<Vec<String>> {
        Some(self.item(at)?.as_array()?.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
    }

    /// The names of the settings and tables in a table, in the file's order.
    pub fn keys(&self, at: &[&str]) -> Vec<String> {
        let table = if at.is_empty() { Some(self.doc.as_table()) } else { self.item(at).and_then(Item::as_table) };
        table.map(|t| t.iter().map(|(k, _)| k.to_owned()).collect()).unwrap_or_default()
    }

    /// The tables of a list of them, `[[servers]]` say, each to be read
    /// as a file of its own.
    pub fn each(&self, at: &[&str]) -> Vec<File> {
        let Some(tables) = self.item(at).and_then(Item::as_array_of_tables) else { return vec![] };
        tables
            .iter()
            .map(|t| {
                let mut doc = DocumentMut::new();
                *doc.as_table_mut() = t.clone();
                File { path: self.path.clone(), doc, problem: None, fresh: false }
            })
            .collect()
    }

    fn table_mut(&mut self, tables: &[&str]) -> &mut Table {
        let mut table = self.doc.as_table_mut();
        for name in tables {
            let item = table.entry(name).or_insert_with(|| Item::Table(Table::new()));
            // Something else of that name is in the way: a table takes its place.
            if !item.is_table() {
                *item = Item::Table(Table::new());
            }
            table = item.as_table_mut().expect("a table was just put there");
        }
        table
    }

    /// Sets a setting, making the tables on the way to it. One that is
    /// already so is left exactly as it is written, and one that changes
    /// keeps the comment beside it.
    pub fn set(&mut self, at: &[&str], value: impl Into<Value>) {
        let Some((last, tables)) = at.split_last() else { return };
        let mut value: Value = value.into();
        let same = |a: &Value, b: &Value| match (a, b) {
            (Value::String(a), Value::String(b)) => a.value() == b.value(),
            (Value::Boolean(a), Value::Boolean(b)) => a.value() == b.value(),
            (Value::Integer(a), Value::Integer(b)) => a.value() == b.value(),
            (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| a.as_str().is_some() && a.as_str() == b.as_str()),
            // 8 and 8.0 are the same setting, however it was written.
            (a, b) => matches!((number_of(a), number_of(b)), (Some(a), Some(b)) if (a - b).abs() < 1e-6),
        };
        let table = self.table_mut(tables);
        match table.get_mut(last).and_then(Item::as_value_mut) {
            Some(old) if same(old, &value) => {}
            Some(old) => {
                *value.decor_mut() = old.decor().clone();
                *old = value;
            }
            None => {
                table.insert(last, Item::Value(value));
            }
        }
    }

    /// Sets a number: written whole if it is, and without the long tail a
    /// number kept in less room would have.
    pub fn set_number(&mut self, at: &[&str], n: f32) {
        let n = (f64::from(n) * 1000.0).round() / 1000.0;
        if n.fract() == 0.0 { self.set(at, n as i64) } else { self.set(at, n) }
    }

    pub fn set_list(&mut self, at: &[&str], list: &[String]) {
        self.set(at, Value::Array(list.iter().map(String::as_str).collect::<Array>()));
    }

    /// Takes a setting, or a whole table, out.
    pub fn remove(&mut self, at: &[&str]) {
        if let Some((last, tables)) = at.split_last()
            && self.item(at).is_some()
        {
            self.table_mut(tables).remove(last);
        }
    }

    /// The file as it would be written.
    pub fn encode(&self) -> String {
        self.doc.to_string()
    }

    /// Writes the file, beside itself and then over itself, so that no
    /// reader sees half of it. One with a mistake in it is the user's to
    /// put right, and is not written over.
    pub fn save(&mut self) -> std::io::Result<()> {
        if let Some(problem) = self.problem() {
            return Err(std::io::Error::other(problem));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = self.encode();
        if self.fresh {
            text.insert_str(0, "# Neo's settings. Neo writes this file and you can too: what is changed here\n# is taken up within a second or so, and what Neo changes leaves your comments be.\n\n");
            // Read back, so that what is written next still has these lines.
            *self = Self { path: std::mem::take(&mut self.path), ..Self::parse(&text) };
        }
        let part = self.path.with_extension("toml.part");
        std::fs::write(&part, text)?;
        std::fs::rename(part, &self.path)
    }
}

fn number_of(v: &Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|n| n as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_found_by_the_way_to_them() {
        let f = File::parse("top = \"level\"\n[appearance]\nscheme = \"dark\"\nradius = 12\ntext-scale = 1.25\nreduce-motion = true\n[appearance.glass]\nopacity = 0.5\n[windows]\nfloat = [\"Notes\", 3, \"Safari\"]\n[[servers]]\ncommand = \"a\"\n[[servers]]\ncommand = \"b\"\n");
        assert_eq!((f.text(&["appearance", "scheme"]), f.number(&["appearance", "radius"]), f.number(&["appearance", "text-scale"]), f.flag(&["appearance", "reduce-motion"])), (Some("dark"), Some(12.0), Some(1.25), Some(true)));
        assert_eq!((f.number(&["appearance", "glass", "opacity"]), f.text(&["top"]), f.list(&["windows", "float"])), (Some(0.5), Some("level"), Some(vec!["Notes".to_owned(), "Safari".to_owned()])));
        // What is not there, or is not the kind of thing asked for, is nothing.
        assert_eq!((f.text(&["appearance", "radius"]), f.number(&["appearance", "scheme"]), f.flag(&["nowhere", "at", "all"]), f.text(&[]), f.text(&["top", "under"])), (None, None, None, None, None));
        assert!(f.has(&["appearance", "glass"]) && f.has(&["windows"]) && !f.has(&["startup"]));
        assert_eq!((f.keys(&["appearance"]), f.keys(&["nowhere"])), (vec!["scheme".to_owned(), "radius".into(), "text-scale".into(), "reduce-motion".into(), "glass".into()], vec![]));
        assert_eq!(f.each(&["servers"]).iter().map(|s| s.text(&["command"]).unwrap().to_owned()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(f.each(&["windows"]).is_empty());
    }

    #[test]
    fn a_change_leaves_the_rest_of_the_file_as_it_was() {
        let mine = "# My desktop.\n\n[appearance]\n# Dark, always.\nscheme = \"dark\"   # not auto\nradius   = 12.0\nmine = \"something Neo does not know\"\n\n[other]\nkept = true\n";
        let mut f = File::parse(mine);
        // Set to what it is already, however that was written, nothing changes.
        f.set(&["appearance", "scheme"], "dark");
        f.set_number(&["appearance", "radius"], 12.0);
        assert_eq!(f.encode(), mine);
        f.set(&["appearance", "scheme"], "light");
        f.set_number(&["appearance", "radius"], 8.0);
        f.set_number(&["appearance", "text-scale"], 1.15);
        f.set(&["appearance", "glass", "enabled"], false);
        f.set_list(&["windows", "float"], &["Notes".to_owned()]);
        assert_eq!(f.encode(), "# My desktop.\n\n[appearance]\n# Dark, always.\nscheme = \"light\"   # not auto\nradius   = 8\nmine = \"something Neo does not know\"\ntext-scale = 1.15\n\n[appearance.glass]\nenabled = false\n\n[other]\nkept = true\n\n[windows]\nfloat = [\"Notes\"]\n");
        f.remove(&["other"]);
        f.remove(&["appearance", "mine"]);
        f.remove(&["not", "there"]);
        assert!(!f.has(&["other"]) && !f.has(&["appearance", "mine"]) && !f.has(&["not"]) && f.has(&["appearance", "scheme"]));
        // A setting where a table is wanted makes way for it.
        f.set(&["appearance", "scheme", "deeper"], 1);
        assert_eq!(f.number(&["appearance", "scheme", "deeper"]), Some(1.0));
    }

    #[test]
    fn a_file_is_written_and_read_back_and_one_with_a_mistake_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("neo-config-{}", std::process::id()));
        let path = dir.join("apps").join("demo.toml");
        let mut f = File::open(path.clone());
        assert!(f.problem().is_none() && !f.has(&["glass"]) && modified(&path).is_none(), "no file yet");
        f.set(&["glass"], false);
        f.save().unwrap();
        f.set(&["more"], "later");
        f.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Neo's settings.") && text.matches("# Neo's settings.").count() == 1 && text.ends_with("glass = false\nmore = \"later\"\n"), "{text}");
        assert_eq!((File::open(path.clone()).flag(&["glass"]), modified(&path).is_some()), (Some(false), true));
        // A mistake made by hand: nothing is read, and the file is not written over.
        let broken = "glass = false\nmore = = \"later\"\n";
        std::fs::write(&path, broken).unwrap();
        let mut f = File::open(path.clone());
        let said = f.problem().unwrap();
        assert!(said.starts_with("demo.toml has a mistake at line 2, column 8"), "{said}");
        f.set(&["glass"], true);
        assert!(f.save().is_err() && f.flag(&["more"]).is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
