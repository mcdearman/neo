//! What Apollo is set to do: which folders it reads, which models it
//! uses, and whether it remembers what it is asked.

use std::path::{Path, PathBuf};

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
}

impl Default for Settings {
    fn default() -> Self {
        let dir = neo_desktop::fs::user_dir;
        let folders = ["PICTURES", "VIDEOS", "DOCUMENTS"].into_iter().map(|d| Folder { path: dir(d), on: true }).collect();
        Self { folders, chat_model: "gemma3:1b".into(), vision_model: "gemma3:4b".into(), embed_model: "nomic-embed-text".into(), server: "http://127.0.0.1:11434".into(), remote: String::new(), chat_remote: false, vision_remote: false, embed_remote: false, remember_conversations: true, read_automatically: true, look_ahead: false }
    }
}

impl Settings {
    pub fn file() -> PathBuf {
        crate::dir().join("settings")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::file())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path).map(|text| Self::decode(&text)).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::file())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.encode())
    }

    pub fn encode(&self) -> String {
        let mut out = format!("chat-model={}\nvision-model={}\nembed-model={}\nserver={}\nremember-conversations={}\nread-automatically={}\nlook-ahead={}\n", self.chat_model, self.vision_model, self.embed_model, self.server, self.remember_conversations, self.read_automatically, self.look_ahead);
        // Said even when there are none, so that none is not read as the defaults.
        out.push_str("folders=\n");
        out.push_str(&format!("remote-server={}\nchat-remote={}\nvision-remote={}\nembed-remote={}\n", self.remote, self.chat_remote, self.vision_remote, self.embed_remote));
        for f in &self.folders {
            out.push_str(&format!("{}={}\n", if f.on { "folder" } else { "folder-off" }, f.path.display()));
        }
        out
    }

    /// Reads what [`encode`](Self::encode) wrote. What is not said keeps
    /// its default, but for the folders: a file that names any names all.
    pub fn decode(text: &str) -> Self {
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
                "server" if !value.is_empty() => s.server = value.trim_end_matches('/').into(),
                "remote-server" => s.remote = value.trim_end_matches('/').into(),
                "chat-remote" => s.chat_remote = value == "true",
                "vision-remote" => s.vision_remote = value == "true",
                "embed-remote" => s.embed_remote = value == "true",
                "remember-conversations" => s.remember_conversations = value != "false",
                "read-automatically" => s.read_automatically = value != "false",
                "look-ahead" => s.look_ahead = value == "true",
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
            server: "http://box:1".into(),
            remote: "http://studio.local:11434".into(),
            chat_remote: true,
            vision_remote: true,
            embed_remote: false,
            remember_conversations: false,
            read_automatically: false,
            look_ahead: true,
        };
        assert_eq!(Settings::decode(&s.encode()), s);
        assert_eq!(s.roots(), [PathBuf::from("/a b/c")]);
        // Nothing said: the defaults. Folders all removed: none, not the defaults back.
        assert_eq!(Settings::decode(""), d);
        let none = Settings { folders: vec![], ..d.clone() };
        assert_eq!(Settings::decode(&none.encode()).folders, vec![]);
    }
}
