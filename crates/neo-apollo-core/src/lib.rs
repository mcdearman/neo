//! Apollo: Neo's assistant, and the memory it keeps.
//!
//! Apollo looks at what is on this computer (pictures, videos, documents,
//! and what it is told) and remembers what each is about, so that it can
//! be asked later: "the video of the dog on the beach", "what did that
//! invoice say". Everything runs here: a model on this computer describes
//! and answers, and what is remembered goes into an encrypted database
//! that is searched by meaning.
//!
//! - [`store`]: the database, SQLite encrypted with SQLCipher, with
//!   sqlite-vec holding an embedding of each memory.
//! - [`key`]: the database's key, kept in the system's keychain, and the
//!   check of the user's login that looking through the memory asks for.
//! - [`model`]: the model that describes, embeds and answers.
//! - [`index`]: reading folders into memories.
//! - [`pressure`]: whether there is memory to spare for reading.
//! - [`words`]: the words that matter in a piece of text, for the cloud.
//! - [`assistant`]: answering a question from what is remembered.
//! - [`settings`]: which folders are read, and which models are used.

pub mod assistant;
pub mod index;
pub mod key;
pub mod model;
pub mod pressure;
pub mod settings;
pub mod store;
pub mod words;

pub use model::Model;
pub use store::{Hit, Kind, Memory, Store};

/// Where Apollo keeps its database and settings.
pub fn dir() -> std::path::PathBuf {
    neo_desktop::config_dir().join("apollo")
}
