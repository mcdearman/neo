//! What NeoCode keeps between runs: settings, and for each folder the
//! files that were open in it.
//!
//! Settings are TOML, as all of Neo's are, with VS Code's names where
//! there is one for the same thing: `tabSize` under `[editor]` is its
//! `editor.tabSize`. The user's are in `apps/neo-code.toml` in Neo's
//! settings folder; a project can carry its own at
//! `.neocode/settings.toml`, which wins for that project. What NeoCode
//! does not know is left alone, comments and all.

use std::path::{Path, PathBuf};

use neo::Syntax;
use neo::prelude::Keymap;
use neo_desktop::config::File;
use serde_json::{Map, Value, json};

/// The settings NeoCode acts on.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub keymap: Keymap,
    pub font_size: f32,
    pub tab_size: usize,
    /// Say what is under the mouse when it rests on a word.
    pub hover: bool,
    /// How long it has to rest first, in milliseconds.
    pub hover_delay: u64,
    /// Show what the language server offers to do above functions.
    pub code_lens: bool,
    /// Dark or light whatever the system is; `None` follows it.
    pub dark: Option<bool>,
    /// Come back to the files that were open in a folder.
    pub restore: bool,
    /// How code is coloured.
    pub syntax: Syntax,
}

impl Default for Settings {
    fn default() -> Self {
        Self { keymap: Keymap::Vim, font_size: 14.0, tab_size: 4, hover: true, hover_delay: 350, code_lens: true, dark: None, restore: true, syntax: Syntax::default() }
    }
}

pub const KEYMAP: &str = "editor.keymap";
pub const THEME: &str = "workbench.colorTheme";
pub const SYNTAX: &str = "editor.colorScheme";

/// How a keymap is written in the file.
pub fn keymap_name(k: Keymap) -> &'static str {
    match k {
        Keymap::Vim => "vim",
        Keymap::Helix => "helix",
        Keymap::Plain => "standard",
    }
}

impl Settings {
    /// Every setting with its usual value, as the file to start from.
    pub fn defaults() -> Map<String, Value> {
        let d = Settings::default();
        let Value::Object(map) = json!({
            KEYMAP: keymap_name(d.keymap),
            "editor.fontSize": d.font_size,
            "editor.tabSize": d.tab_size,
            "editor.hover.enabled": d.hover,
            "editor.hover.delay": d.hover_delay,
            "editor.codeLens": d.code_lens,
            THEME: "system",
            SYNTAX: d.syntax.name().to_ascii_lowercase(),
            "window.restoreWorkspace": d.restore,
        }) else {
            unreachable!("an object was written")
        };
        map
    }

    /// Reads settings out of a file's values. Anything of the wrong kind
    /// or out of range keeps its usual value and is named in the second
    /// result; keys that are not NeoCode's are passed over.
    pub fn from(values: &Map<String, Value>) -> (Self, Vec<String>) {
        let mut s = Settings::default();
        let mut wrong = vec![];
        for (key, value) in values {
            let ok = match key.as_str() {
                KEYMAP => value
                    .as_str()
                    .and_then(|v| match v.to_ascii_lowercase().as_str() {
                        "vim" => Some(Keymap::Vim),
                        "helix" => Some(Keymap::Helix),
                        "standard" | "plain" | "default" => Some(Keymap::Plain),
                        _ => None,
                    })
                    .map(|k| s.keymap = k),
                "editor.fontSize" => value.as_f64().filter(|n| (6.0..=72.0).contains(n)).map(|n| s.font_size = n as f32),
                "editor.tabSize" => value.as_u64().filter(|n| (1..=16).contains(n)).map(|n| s.tab_size = n as usize),
                "editor.hover.enabled" => value.as_bool().map(|b| s.hover = b),
                "editor.hover.delay" => value.as_u64().filter(|n| *n <= 10_000).map(|n| s.hover_delay = n),
                "editor.codeLens" => value.as_bool().map(|b| s.code_lens = b),
                THEME => value
                    .as_str()
                    .and_then(|v| match v.to_ascii_lowercase().as_str() {
                        "dark" => Some(Some(true)),
                        "light" => Some(Some(false)),
                        "system" | "auto" => Some(None),
                        _ => None,
                    })
                    .map(|dark| s.dark = dark),
                "window.restoreWorkspace" => value.as_bool().map(|b| s.restore = b),
                SYNTAX => value.as_str().and_then(Syntax::from_name).map(|x| s.syntax = x),
                _ => Some(()),
            };
            if ok.is_none() {
                wrong.push(key.clone());
            }
        }
        (s, wrong)
    }
}

/// JSON with the comments taken out, since people wrote them in the
/// `settings.json` these were kept in before. Text inside strings is left alone.
fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == '/' && chars.peek() == Some(&'/') {
            // To the end of the line, which is kept so line numbers hold.
            while chars.next_if(|c| *c != '\n').is_some() {}
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut last = ' ';
            for c in chars.by_ref() {
                if last == '*' && c == '/' {
                    break;
                }
                if c == '\n' {
                    out.push('\n');
                }
                last = c;
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The values in a settings file, each under its whole name:
/// `editor.tabSize` for `tabSize` under `[editor]`. A file that is not
/// there has none; one that cannot be read as settings says what is
/// wrong with it.
pub fn read(file: &Path) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(format!("Couldn't read {}: {e}", file.display())),
    };
    if let Some(problem) = File::open(file.to_owned()).problem() {
        return Err(problem);
    }
    let mut values = Map::new();
    if let Ok(doc) = text.parse::<toml_edit::DocumentMut>() {
        flatten("", doc.as_table(), &mut values);
    }
    Ok(values)
}

/// Puts a table's settings into `out` under their whole names. Lists of
/// tables, the user's language servers say, are not settings of this kind.
fn flatten(under: &str, table: &toml_edit::Table, out: &mut Map<String, Value>) {
    fn plain(v: &toml_edit::Value) -> Option<Value> {
        Some(match v {
            toml_edit::Value::String(s) => json!(s.value()),
            toml_edit::Value::Integer(n) => json!(n.value()),
            toml_edit::Value::Float(n) => json!(n.value()),
            toml_edit::Value::Boolean(b) => json!(b.value()),
            toml_edit::Value::Array(a) => Value::Array(a.iter().filter_map(plain).collect()),
            _ => return None,
        })
    }
    for (key, item) in table.iter() {
        let name = if under.is_empty() { key.to_owned() } else { format!("{under}.{key}") };
        match item {
            toml_edit::Item::Table(t) => flatten(&name, t, out),
            toml_edit::Item::Value(v) => {
                if let Some(v) = plain(v) {
                    out.insert(name, v);
                }
            }
            _ => {}
        }
    }
}

/// Puts one value into a file under its whole name.
fn put(file: &mut File, key: &str, value: &Value) {
    let at: Vec<&str> = key.split('.').collect();
    match value {
        Value::String(s) => file.set(&at, s.as_str()),
        Value::Bool(b) => file.set(&at, *b),
        Value::Number(n) => match n.as_i64() {
            Some(whole) => file.set(&at, whole),
            None => file.set_number(&at, n.as_f64().unwrap_or(0.0) as f32),
        },
        _ => {}
    }
}

/// Changes one setting in a file, keeping the rest as it is written. A
/// file with a mistake in it is left as it is rather than written over.
pub fn set(file: &Path, key: &str, value: Value) -> Result<(), String> {
    let mut toml = File::open(file.to_owned());
    if let Some(problem) = toml.problem() {
        return Err(problem);
    }
    put(&mut toml, key, &value);
    toml.save().map_err(|e| format!("Couldn't write {}: {e}", file.display()))
}

/// Puts every setting the file does not have into it, with its usual
/// value, so that opening it shows what there is to change.
pub fn ensure(file: &Path) -> std::io::Result<()> {
    let mut toml = File::open(file.to_owned());
    for (key, value) in Settings::defaults() {
        if !toml.has(&key.split('.').collect::<Vec<_>>()) {
            put(&mut toml, &key, &value);
        }
    }
    toml.save()
}

/// The user's settings file, for NeoCode's own folder `dir`: beside it,
/// in `apps`, with the other apps' settings, as `apps/neo-code.toml`.
pub fn user_file(dir: &Path) -> PathBuf {
    let name = dir.file_name().map_or_else(|| "neo-code".into(), |n| n.to_string_lossy().into_owned());
    dir.with_file_name("apps").join(format!("{name}.toml"))
}

/// A project's own settings file.
pub fn folder_file(root: &Path) -> PathBuf {
    root.join(".neocode").join("settings.toml")
}

/// The settings that were kept as JSON before, in `settings.json`.
fn read_json(file: &Path) -> Option<Map<String, Value>> {
    match serde_json::from_str(&without_comments(&std::fs::read_to_string(file).ok()?)) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// Whether a name is one of NeoCode's settings, and not something else
/// kept in the same file.
fn ours(key: &str) -> bool {
    ["editor.", "workbench.", "window."].iter().any(|p| key.starts_with(p))
}

/// The settings in force: the user's, with the project's over them. Says
/// what was wrong with either file, if anything.
pub fn load(dir: Option<&Path>, root: Option<&Path>) -> (Settings, Vec<String>) {
    let mut problems = vec![];
    let mut values = Map::new();
    if let Some(dir) = dir {
        let file = user_file(dir);
        match read(&file) {
            Ok(mut found) => {
                // Kept as JSON in NeoCode's own folder before: brought over, the once.
                if !found.keys().any(|k| ours(k))
                    && let Some(old) = read_json(&dir.join("settings.json"))
                {
                    let mut toml = File::open(file.clone());
                    for (key, value) in &old {
                        put(&mut toml, key, value);
                    }
                    let _ = toml.save();
                    found = read(&file).unwrap_or(old);
                }
                values.extend(found);
            }
            Err(why) => problems.push(why),
        }
    }
    if let Some(root) = root {
        let file = folder_file(root);
        // A project that still carries the JSON one is read as it is, and not written to.
        let found = if file.exists() { read(&file) } else { Ok(read_json(&file.with_extension("json")).unwrap_or_default()) };
        match found {
            Ok(more) => values.extend(more),
            Err(why) => problems.push(why),
        }
    }
    values.retain(|key, _| ours(key));
    let (settings, wrong) = Settings::from(&values);
    problems.extend(wrong.into_iter().map(|key| format!("\"{key}\" in the settings has a value NeoCode can't use.")));
    (settings, problems)
}

/// What was open in a folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workspace {
    /// The open files in tab order, by their paths within the folder,
    /// each with where the caret was: line and column.
    pub open: Vec<(String, usize, usize)>,
    /// Which of them was showing.
    pub active: Option<String>,
}

fn workspace_file(dir: &Path, root: &Path) -> PathBuf {
    // Named by a hash of the path, the same on every run.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.as_os_str().as_encoded_bytes() {
        hash = (hash ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    dir.join("workspaces").join(format!("{hash:016x}.json"))
}

pub fn save_workspace(dir: &Path, root: &Path, ws: &Workspace) -> std::io::Result<()> {
    let file = workspace_file(dir, root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let open: Vec<Value> = ws.open.iter().map(|(path, line, col)| json!({ "path": path, "line": line, "column": col })).collect();
    let text = serde_json::to_string_pretty(&json!({ "folder": root.to_string_lossy(), "open": open, "active": ws.active })).map_err(std::io::Error::other)?;
    std::fs::write(file, text + "\n")
}

pub fn load_workspace(dir: &Path, root: &Path) -> Option<Workspace> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(workspace_file(dir, root)).ok()?).ok()?;
    // Two folders could share a file name; only the right one will do.
    if value["folder"].as_str() != Some(&*root.to_string_lossy()) {
        return None;
    }
    let open = value["open"].as_array()?.iter().filter_map(|f| Some((f["path"].as_str()?.to_owned(), f["line"].as_u64().unwrap_or(0) as usize, f["column"].as_u64().unwrap_or(0) as usize))).collect();
    Some(Workspace { open, active: value["active"].as_str().map(str::to_owned) })
}

/// How many folders are remembered.
const RECENT: usize = 8;

/// The folders opened lately, the latest first.
pub fn recent(dir: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(dir.join("recent.json")) else { return vec![] };
    serde_json::from_str::<Vec<String>>(&text).unwrap_or_default().into_iter().map(PathBuf::from).collect()
}

/// Puts a folder at the front of those opened lately.
pub fn add_recent(dir: &Path, root: &Path) -> std::io::Result<()> {
    let mut list = recent(dir);
    list.retain(|p| p != root);
    list.insert(0, root.to_path_buf());
    list.truncate(RECENT);
    std::fs::create_dir_all(dir)?;
    let names: Vec<String> = list.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    std::fs::write(dir.join("recent.json"), serde_json::to_string_pretty(&names).map_err(std::io::Error::other)? + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neo-code-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn settings_are_read_by_name_and_bad_values_keep_the_usual_one() {
        let Value::Object(values) = json!({ "editor.keymap": "Helix", "editor.fontSize": 16.5, "editor.tabSize": 2, "editor.hover.enabled": false, "editor.hover.delay": 100, "editor.codeLens": false, "workbench.colorTheme": "dark", "window.restoreWorkspace": false, "editor.colorScheme": "meadow", "someone.elses": [1, 2] }) else { panic!() };
        let (s, wrong) = Settings::from(&values);
        assert_eq!(s, Settings { keymap: Keymap::Helix, font_size: 16.5, tab_size: 2, hover: false, hover_delay: 100, code_lens: false, dark: Some(true), restore: false, syntax: Syntax::Meadow });
        assert!(wrong.is_empty(), "a key that is not NeoCode's is nobody's mistake: {wrong:?}");
        let Value::Object(values) = json!({ "editor.colorScheme": "Monokai" }) else { panic!() };
        assert_eq!(Settings::from(&values).0.syntax, Syntax::Monokai);
        let Value::Object(values) = json!({ "editor.keymap": "emacs", "editor.fontSize": 400, "editor.tabSize": "two", "workbench.colorTheme": 3, "editor.colorScheme": "plaid" }) else { panic!() };
        let (s, mut wrong) = Settings::from(&values);
        wrong.sort();
        assert_eq!(s, Settings::default());
        assert_eq!(wrong, ["editor.colorScheme", "editor.fontSize", "editor.keymap", "editor.tabSize", "workbench.colorTheme"]);
        // The file to start from says what the usual values are.
        assert_eq!(Settings::from(&Settings::defaults()), (Settings::default(), vec![]));
        assert_eq!(Settings::defaults().len(), 9);
    }

    #[test]
    fn a_settings_file_may_have_comments_and_a_broken_one_says_where() {
        let dir = scratch("read");
        let file = user_file(&dir);
        assert_eq!(read(&file), Ok(Map::new()), "no file is no settings, not a mistake");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "# mine\nglass = false\nnote = \"not # a comment\"\n\n[editor]\ntabSize = 2 # two spaces\nfontSize = 13.5\n\n[editor.hover]\nenabled = false\n\n[[servers]]\ncommand = \"not a setting\"\n").unwrap();
        let values = read(&file).unwrap();
        assert_eq!((values["editor.tabSize"].clone(), values["note"].clone(), values["editor.hover.enabled"].clone(), values["editor.fontSize"].clone(), values.len()), (json!(2), json!("not # a comment"), json!(false), json!(13.5), 5));
        std::fs::write(&file, "[editor]\ntabSize = 2\noops\n").unwrap();
        let why = read(&file).unwrap_err();
        assert!(why.contains("mistake at line 3"), "{why}");
        // A file with a mistake is not written over when a setting changes.
        assert!(set(&file, KEYMAP, json!("vim")).is_err());
        assert!(std::fs::read_to_string(&file).unwrap().contains("oops"));
        // The JSON these were kept in before could have comments, and is still read.
        std::fs::write(dir.join("settings.json"), "// mine\n{\n  \"editor.tabSize\": 2, /* two\n spaces */\n  \"note\": \"not // a comment\"\n}\n").unwrap();
        let old = read_json(&dir.join("settings.json")).unwrap();
        assert_eq!((old["editor.tabSize"].clone(), old["note"].clone()), (json!(2), json!("not // a comment")));
        std::fs::write(dir.join("settings.json"), "[1]").unwrap();
        assert!(read_json(&dir.join("settings.json")).is_none());
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn changing_one_setting_keeps_the_rest_and_the_project_wins() {
        let dir = scratch("set");
        let root = scratch("set-project");
        let file = user_file(&dir);
        ensure(&file).unwrap();
        assert_eq!(Settings::from(&read(&file).unwrap()), (Settings::default(), vec![]));
        assert_eq!(read(&file).unwrap().len(), Settings::defaults().len());
        std::fs::write(&file, "[someone]\nelses = true # theirs\n\n[editor]\ntabSize = 8\n").unwrap();
        ensure(&file).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("[someone]\nelses = true # theirs\n\n[editor]\ntabSize = 8\n"), "what was there is as it was, with the rest after it");
        set(&file, KEYMAP, json!("helix")).unwrap();
        let values = read(&file).unwrap();
        assert_eq!((values["someone.elses"].clone(), values["editor.tabSize"].clone(), values[KEYMAP].clone()), (json!(true), json!(8), json!("helix")));
        let (s, problems) = load(Some(&dir), Some(&root));
        assert_eq!((s.keymap, s.tab_size, problems.len()), (Keymap::Helix, 8, 0));
        // The project's own file is laid over the user's.
        std::fs::create_dir_all(root.join(".neocode")).unwrap();
        std::fs::write(folder_file(&root), "[editor]\ntabSize = 2\nfontSize = \"big\"\n").unwrap();
        let (s, problems) = load(Some(&dir), Some(&root));
        assert_eq!((s.keymap, s.tab_size), (Keymap::Helix, 2));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(load(None, None), (Settings::default(), vec![]));
        std::fs::remove_file(&file).unwrap();
        for d in [dir, root] {
            std::fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn a_folders_open_files_and_the_folders_opened_lately_are_kept() {
        let dir = scratch("workspace");
        let (a, b) = (Path::new("/work/alpha"), Path::new("/work/beta"));
        assert_eq!(load_workspace(&dir, a), None);
        let ws = Workspace { open: vec![("src/main.rs".into(), 12, 4), ("README.md".into(), 0, 0)], active: Some("README.md".into()) };
        save_workspace(&dir, a, &ws).unwrap();
        assert_eq!(load_workspace(&dir, a), Some(ws));
        assert_eq!(load_workspace(&dir, b), None, "each folder has its own");
        save_workspace(&dir, a, &Workspace::default()).unwrap();
        assert_eq!(load_workspace(&dir, a), Some(Workspace::default()), "closing everything is remembered too");
        // Lately opened: the latest first, each once, and not for ever.
        assert!(recent(&dir).is_empty());
        add_recent(&dir, a).unwrap();
        add_recent(&dir, b).unwrap();
        add_recent(&dir, a).unwrap();
        assert_eq!(recent(&dir), [a, b]);
        for i in 0..20 {
            add_recent(&dir, Path::new(&format!("/work/{i}"))).unwrap();
        }
        assert_eq!(recent(&dir).len(), RECENT);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
