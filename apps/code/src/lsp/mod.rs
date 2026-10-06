//! Language servers: finding them on this computer and talking to them.

pub mod client;
pub mod servers;

pub use client::{apply_edits, hover_lines, to_place, to_pos, Client, Completion, Diagnostic, Event, Incoming, Severity};
pub use servers::{all, find_in, search_dirs, spec_for, user_servers_file, Found, ServerSpec};

#[cfg(test)]
pub mod testing {
    //! A stand-in for a server's input, for tests.

    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use serde_json::Value;

    /// Collects what a client writes, to read back as messages.
    #[derive(Clone, Default)]
    pub struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Sink {
        /// The messages written since the last call.
        pub fn take(&self) -> Vec<Value> {
            let bytes = std::mem::take(&mut *self.0.lock().unwrap());
            let mut reader = std::io::BufReader::new(&bytes[..]);
            let mut out = vec![];
            while let Ok(Some(m)) = super::client::read_message(&mut reader) {
                out.push(m);
            }
            out
        }
    }
}
