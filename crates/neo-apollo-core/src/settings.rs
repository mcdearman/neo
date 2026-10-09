//! What Apollo is set to do: which folders it reads, which models it
//! uses, and whether it remembers what it is asked.

use std::path::{Path, PathBuf};

use neo_desktop::config::File;

/// A folder Apollo reads, or has been told to leave for now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub path: PathBuf,
    pub on: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub folders: Vec<Folder>,
    /// The model that answers: a small one, so that Apollo can wait to be
    /// asked without holding much, and be ready in a second or two.
    pub chat_model: String,
    /// The model that describes pictures: one that can see. Larger, and in
    /// memory only while the folders are being read.
    pub vision_model: String,
    /// The model that turns text into embeddings.
    pub embed_model: String,
    /// The model that writes down what is said: one of `hear::LISTENERS`, by its id.
    pub hear_model: String,
    /// Where the model server on this computer listens.
    pub server: String,
    /// The address of a model server on another computer, for models too
    /// large for this one. Empty if there is none.
    pub remote: String,
    /// Whether each job is done on the other computer.
    pub chat_remote: bool,
    pub vision_remote: bool,
    pub embed_remote: bool,
    /// Whether what is asked and answered is remembered.
    pub remember_conversations: bool,
    /// Whether the folders are read without being asked: when Apollo
    /// starts, and now and then while it is open.
    pub read_automatically: bool,
    /// Whether every photo and video is looked at up front. Otherwise they
    /// are listed by name at once, which is quick, and looked at when a
    /// question or a search turns them up.
    pub look_ahead: bool,
    /// Whether Apollo warns, through NeoShell, of memory or disk running
    /// out and of what stands open.
    pub warn: bool,
    /// Whether Apollo stays running out of sight when its window is
    /// closed, with the model that answers kept in memory, so that a
    /// question from the search bar is answered at once.
    pub background: bool,
    /// How long the memory stays unlocked after it was last used, in
    /// minutes. Nothing, to stay unlocked until Apollo quits.
    pub stay_unlocked: u32,
}

impl Default for Settings {
    fn default() -> Self {
        let dir = neo_desktop::fs::user_dir;
        let folders = ["PICTURES", "VIDEOS", "DOCUMENTS"].into_iter().map(|d| Folder { path: dir(d), on: true }).collect();
        Self {
            folders,
            chat_model: "gemma3:1b".into(),
            vision_model: "gemma3:4b".into(),
            embed_model: "nomic-embed-text".into(),
            hear_model: "turbo".into(),
            server: "http://127.0.0.1:11434".into(),
            remote: String::new(),
            chat_remote: false,
            vision_remote: false,
            embed_remote: false,
            remember_conversations: true,
            read_automatically: true,
            look_ahead: false,
            warn: true,
            background: true,
            stay_unlocked: 15,
        }
    }
}

impl Settings {
    /// The file these are kept in: `apps/neo-apollo.toml`.
    pub fn file() -> PathBuf {
        neo_desktop::config::app_path("neo-apollo")
    }

    pub fn load() -> Self {
        let path = Self::file();
        // Kept in Apollo's own folder before: brought over, the once.
        if !path.exists()
            && let Ok(old) = std::fs::read_to_string(crate::dir().join("settings"))
        {
            let s = Self::legacy(&old);
            let _ = s.save_to(&path);
            return s;
        }
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Self {
        Self::read(&File::open(path.to_owned()))
    }

    /// As [`load`](Self::load), or what is wrong with the file: a mistake
    /// made editing it by hand is not to put everything back to defaults.
    pub fn try_load() -> Result<Self, String> {
        match File::open(Self::file()).problem() {
            Some(problem) => Err(problem),
            None => Ok(Self::load()),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::file())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let mut file = File::open(path.to_owned());
        self.write(&mut file);
        file.save()
    }

    /// Puts the settings into a file, leaving whatever else is in it.
    pub fn write(&self, file: &mut File) {
        let paths = |on: bool| self.folders.iter().filter(|f| f.on == on).map(|f| f.path.display().to_string()).collect::<Vec<_>>();
        // Said even when there are none, so that none is not read as the defaults.
        file.set_list(&["folders"], &paths(true));
        file.set_list(&["folders-off"], &paths(false));
        file.set(&["remember-conversations"], self.remember_conversations);
        file.set(&["read-automatically"], self.read_automatically);
        file.set(&["look-ahead"], self.look_ahead);
        file.set(&["warnings"], self.warn);
        file.set(&["background"], self.background);
        file.set(&["stay-unlocked-minutes"], i64::from(self.stay_unlocked));
        file.set(&["models", "chat"], self.chat_model.as_str());
        file.set(&["models", "vision"], self.vision_model.as_str());
        file.set(&["models", "embed"], self.embed_model.as_str());
        file.set(&["models", "hear"], self.hear_model.as_str());
        file.set(&["servers", "local"], self.server.as_str());
        file.set(&["servers", "remote"], self.remote.as_str());
        file.set(&["servers", "chat-remote"], self.chat_remote);
        file.set(&["servers", "vision-remote"], self.vision_remote);
        file.set(&["servers", "embed-remote"], self.embed_remote);
    }

    /// The settings in a file. What is not said keeps its default, but
    /// for the folders: a file that names any names all.
    pub fn read(file: &File) -> Self {
        let mut s = Self::default();
        let said = |at: &[&str], into: &mut String| {
            if let Some(v) = file.text(at).map(str::trim).filter(|v| !v.is_empty()) {
                *into = v.to_owned();
            }
        };
        said(&["models", "chat"], &mut s.chat_model);
        said(&["models", "vision"], &mut s.vision_model);
        said(&["models", "embed"], &mut s.embed_model);
        said(&["models", "hear"], &mut s.hear_model);
        said(&["servers", "local"], &mut s.server);
        s.server = s.server.trim_end_matches('/').to_owned();
        s.remote = file.text(&["servers", "remote"]).map_or(s.remote, |v| v.trim().trim_end_matches('/').to_owned());
        s.chat_remote = file.flag(&["servers", "chat-remote"]).unwrap_or(s.chat_remote);
        s.vision_remote = file.flag(&["servers", "vision-remote"]).unwrap_or(s.vision_remote);
        s.embed_remote = file.flag(&["servers", "embed-remote"]).unwrap_or(s.embed_remote);
        s.remember_conversations = file.flag(&["remember-conversations"]).unwrap_or(s.remember_conversations);
        s.read_automatically = file.flag(&["read-automatically"]).unwrap_or(s.read_automatically);
        s.look_ahead = file.flag(&["look-ahead"]).unwrap_or(s.look_ahead);
        s.warn = file.flag(&["warnings"]).unwrap_or(s.warn);
        s.background = file.flag(&["background"]).unwrap_or(s.background);
        s.stay_unlocked = file.number(&["stay-unlocked-minutes"]).map_or(s.stay_unlocked, |n| n.clamp(0.0, 100_000.0) as u32);
        if file.has(&["folders"]) || file.has(&["folders-off"]) {
            s.folders.clear();
            for (key, on) in [("folders", true), ("folders-off", false)] {
                for path in file.list(&[key]).unwrap_or_default() {
                    if !path.is_empty() && !s.folders.iter().any(|f| f.path == Path::new(&path)) {
                        s.folders.push(Folder { path: path.into(), on });
                    }
                }
            }
        }
        s
    }

    /// The settings as a file of their own would have them.
    pub fn encode(&self) -> String {
        let mut file = File::parse("");
        self.write(&mut file);
        file.encode()
    }

    /// The settings in a file's text.
    pub fn decode(text: &str) -> Self {
        Self::read(&File::parse(text))
    }

    /// Reads the `key=value` lines these were kept in before, in a file
    /// called `settings` in Apollo's folder.
    fn legacy(text: &str) -> Self {
        let mut s = Self::default();
        let mut folders = vec![];
        let mut named = false;
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "chat-model" if !value.is_empty() => s.chat_model = value.into(),
                "vision-model" if !value.is_empty() => s.vision_model = value.into(),
                "embed-model" if !value.is_empty() => s.embed_model = value.into(),
                "hear-model" if !value.is_empty() => s.hear_model = value.into(),
                "server" if !value.is_empty() => s.server = value.trim_end_matches('/').into(),
                "remote-server" => s.remote = value.trim_end_matches('/').into(),
                "chat-remote" => s.chat_remote = value == "true",
                "vision-remote" => s.vision_remote = value == "true",
                "embed-remote" => s.embed_remote = value == "true",
                "remember-conversations" => s.remember_conversations = value != "false",
                "read-automatically" => s.read_automatically = value != "false",
                "look-ahead" => s.look_ahead = value == "true",
                "warnings" => s.warn = value != "false",
                "background" => s.background = value != "false",
                "stay-unlocked-minutes" => s.stay_unlocked = value.parse().unwrap_or(s.stay_unlocked),
                "folder" | "folder-off" => {
                    named = true;
                    if !value.is_empty() && !folders.iter().any(|f: &Folder| f.path == Path::new(value)) {
                        folders.push(Folder { path: value.into(), on: key.trim() == "folder" });
                    }
                }
                "folders" => named = true,
                _ => {}
            }
        }
        if named {
            s.folders = folders;
        }
        s
    }

    /// The folders that are read.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.folders.iter().filter(|f| f.on).map(|f| f.path.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_written_and_read_back() {
        let d = Settings::default();
        assert_eq!((d.folders.len(), d.folders.iter().all(|f| f.on), d.remember_conversations), (3, true, true));
        let s = Settings {
            folders: vec![Folder { path: "/a b/c".into(), on: true }, Folder { path: "/d".into(), on: false }],
            chat_model: "qwen3".into(),
            vision_model: "llava".into(),
            embed_model: "embed".into(),
            hear_model: "turbo".into(),
            server: "http://box:1".into(),
            remote: "http://studio.local:11434".into(),
            chat_remote: true,
            vision_remote: true,
            embed_remote: false,
            remember_conversations: false,
            read_automatically: false,
            look_ahead: true,
            warn: false,
            background: false,
            stay_unlocked: 60,
        };
        assert_eq!(Settings::decode(&s.encode()), s);
        assert_eq!(s.roots(), [PathBuf::from("/a b/c")]);
        // Nothing said: the defaults. Folders all removed: none, not the defaults back.
        assert_eq!(Settings::decode(""), d);
        let none = Settings { folders: vec![], ..d.clone() };
        assert_eq!(Settings::decode(&none.encode()).folders, vec![]);
        // By hand: a few lines are enough, and the rest keep their defaults.
        let few = Settings::decode("folders = [\"/music\"]\nstay-unlocked-minutes = 5\n[models]\nchat = \"qwen3:4b\"\n[servers]\nlocal = \"http://box:1/\"\n");
        assert_eq!((few.roots(), few.stay_unlocked, few.chat_model.as_str(), few.server.as_str(), few.vision_model == d.vision_model), (vec![PathBuf::from("/music")], 5, "qwen3:4b", "http://box:1", true));
        // What was kept before is read as it was.
        assert_eq!(
            Settings::legacy(
                "chat-model=qwen3\nvision-model=llava\nembed-model=embed\nserver=http://box:1\nremember-conversations=false\nread-automatically=false\nlook-ahead=true\nwarnings=false\nbackground=false\nstay-unlocked-minutes=60\nfolders=\nremote-server=http://studio.local:11434\nchat-remote=true\nvision-remote=true\nembed-remote=false\nhear-model=turbo\nfolder=/a b/c\nfolder-off=/d\n"
            ),
            s
        );
    }
}
