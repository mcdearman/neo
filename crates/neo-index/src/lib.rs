//! An index of what is on a disk.
//!
//! [`scan`] reads everything under a folder, quickly, into an
//! [`scan::Index`]: every folder with the files in it, by name and size.
//! [`store`] keeps one between runs, and [`scan::refresh`] brings a kept
//! one up to date by reading again only the folders that changed.
//!
//! It is shared. NeoShell, which starts with the computer, keeps the
//! indexes of its disks up to date; NeoDisk shows from one what is using
//! the space, and Apollo finds in one what there is to read. An app that
//! finds none kept, or wants it fresher, makes or refreshes it itself and
//! keeps it for the others.

#[cfg(target_os = "macos")]
mod bulk;
pub mod changes;
pub mod keep;
pub mod scan;
pub mod store;
