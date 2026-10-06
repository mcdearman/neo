//! What NeoCode keeps between runs: settings, and for each folder the
//! files that were open in it.
//!
//! Settings are a `settings.json`, as in VS Code and with its names where
//! there is one for the same thing. The user's is in NeoCode's own
//! folder; a project can carry another at `.neocode/settings.json`,
//! which wins for that project. Keys NeoCode does not know are left
//! alone, so a file can be shared or carried forward.

use std::path::{Path, PathBuf};

use neo::Syntax;
use neo::prelude::Keymap;
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

/// JSON with the comments taken out, since people write them in settings
/// files and VS Code allows it. Text inside strings is left alone.
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

/// The values in a settings file. A file that is not there has none; one
/// that cannot be read as settings says what is wrong with it.
pub fn read(file: &Path) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(format!("Couldn't read {}: {e}", file.display())),
    };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(&without_comments(&text)) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err("settings.json should be one object, in curly brackets.".into()),
        Err(e) => Err(format!("settings.json has a mistake on line {}: {e}", e.line())),
    }
}

fn write(file: &Path, values: &Map<String, Value>) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(values).map_err(std::io::Error::other)?;
    std::fs::write(file, text + "\n")
}

/// Changes one setting in a file, keeping the rest. A file with a mistake
/// in it is left as it is rather than written over.
pub fn set(file: &Path, key: &str, value: Value) -> Result<(), String> {
    let mut values = read(file)?;
    values.insert(key.to_owned(), value);
    write(file, &values).map_err(|e| format!("Couldn't write {}: {e}", file.display()))
}

/// Makes the settings file if there is none, with every setting in it.
pub fn ensure(file: &Path) -> std::io::Result<()> {
    if file.exists() { Ok(()) } else { write(file, &Settings::defaults()) }
}

/// The user's settings file, under NeoCode's own folder `dir`.
pub fn user_file(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

/// A project's own settings file.
pub fn folder_file(root: &Path) -> PathBuf {
    root.join(".neocode").join("settings.json")
}

/// The settings in force: the user's, with the project's over them. Says
/// what was wrong with either file, if anything.
pub fn load(dir: Option<&Path>, root: Option<&Path>) -> (Settings, Vec<String>) {
    let mut problems = vec![];
    let mut values = Map::new();
    for file in dir.map(user_file).into_iter().chain(root.map(folder_file)) {
        match read(&file) {
            Ok(more) => values.extend(more),
            Err(why) => problems.push(why),
        }
    }
    let (settings, wrong) = Settings::from(&values);
    problems.extend(wrong.into_iter().map(|key| format!("\"{key}\" in settings.json has a value NeoCode can't use.")));
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
        std::fs::write(&file, "// mine\n{\n  \"editor.tabSize\": 2, /* two\n spaces */\n  \"note\": \"not // a comment\"\n}\n").unwrap();
        let values = read(&file).unwrap();
        assert_eq!((values["editor.tabSize"].clone(), values["note"].clone()), (json!(2), json!("not // a comment")));
        std::fs::write(&file, "{\n  \"editor.tabSize\": 2,\n  oops\n}\n").unwrap();
        let why = read(&file).unwrap_err();
        assert!(why.contains("line 3"), "{why}");
        // A file with a mistake is not written over when a setting changes.
        assert!(set(&file, KEYMAP, json!("vim")).is_err());
        assert!(std::fs::read_to_string(&file).unwrap().contains("oops"));
        std::fs::write(&file, "[1]").unwrap();
        assert!(read(&file).unwrap_err().contains("curly brackets"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn changing_one_setting_keeps_the_rest_and_the_project_wins() {
        let dir = scratch("set");
        let root = scratch("set-project");
        let file = user_file(&dir);
        ensure(&file).unwrap();
        assert_eq!(read(&file).unwrap(), Settings::defaults());
        std::fs::write(&file, "{ \"someone.elses\": true, \"editor.tabSize\": 8 }").unwrap();
        ensure(&file).unwrap();
        set(&file, KEYMAP, json!("helix")).unwrap();
        let values = read(&file).unwrap();
        assert_eq!((values["someone.elses"].clone(), values["editor.tabSize"].clone(), values[KEYMAP].clone()), (json!(true), json!(8), json!("helix")));
        let (s, problems) = load(Some(&dir), Some(&root));
        assert_eq!((s.keymap, s.tab_size, problems.len()), (Keymap::Helix, 8, 0));
        // The project's own file is laid over the user's.
        std::fs::create_dir_all(root.join(".neocode")).unwrap();
        std::fs::write(folder_file(&root), "{ \"editor.tabSize\": 2, \"editor.fontSize\": \"big\" }").unwrap();
        let (s, problems) = load(Some(&dir), Some(&root));
        assert_eq!((s.keymap, s.tab_size), (Keymap::Helix, 2));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(load(None, None), (Settings::default(), vec![]));
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
