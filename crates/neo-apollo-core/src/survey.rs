//! What there is to read, from the kept index of a disk.
//!
//! Apollo used to find its files by walking the folders itself and then
//! treating every file alike. The index NeoShell keeps of each disk (see
//! `neo-index`) already names every folder and file on it, so the first
//! look at a disk is a look through that: in a moment it says what there
//! is to read and, as much to the point, what is not worth reading file
//! by file. A folder that holds a program or a game is one thing to
//! know of, however many thousand sounds and pictures it is made of: it
//! is remembered as a whole, by what it is and how much is in it, and
//! not gone into.
//!
//! This is the first, coarsest pass of several, each over everything and
//! each adding detail: names and folders here, then what documents say
//! and which pictures there are, then what the pictures show.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use neo_index::keep;
use neo_index::scan::{Folder, Index, Progress};

use crate::index::{self, What};

/// Files a program is made of and a person's own things are not. One of
/// these beside a program to run, or several together, marks the folder
/// they are in as a program's.
const WORKINGS: &[&str] = &["dll", "so", "dylib", "pak", "pck", "vpk", "asar"];
/// A program to run.
const RUNS: &[&str] = &["exe"];
/// An index older than this is brought up to date before it is trusted:
/// NeoShell does so every quarter of an hour while it runs.
const FRESH_FOR: Duration = Duration::from_secs(20 * 60);

/// Why a folder is known of whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Whole {
    /// It holds a program or a game.
    #[default]
    Program,
    /// It is a heap of sound files: hundreds or thousands in one folder,
    /// as a game's sounds are kept, or what a recovery tool dug up. A
    /// person's own recordings and music are not kept so.
    Sounds,
}

/// More sound files than this directly in one folder make it a heap.
const HEAP: u64 = 300;

/// A folder that is known of whole and not read file by file, with what
/// is in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    pub path: PathBuf,
    pub what: Whole,
    /// Everything under it, and what that comes to in bytes.
    pub files: u64,
    pub bytes: u64,
    /// Of those, the ones Apollo would otherwise have read.
    pub sounds: u64,
    pub pictures: u64,
    pub videos: u64,
    pub documents: u64,
}

/// What a look through the indexes found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Survey {
    /// Every file to read: documents, then photos, videos and recordings.
    pub files: Vec<PathBuf>,
    /// The folders that are remembered whole and not read file by file.
    pub programs: Vec<Program>,
}

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Whether a folder is to be known of whole, and why: by what lies
/// directly in it. A program's folder has a program to run with its
/// workings beside it, or several workings, and they are a fair part of
/// what is there: a great heap of every kind of file with a few of them
/// in it, as a recovery tool leaves, is not a program.
fn whole(f: &Folder) -> Option<Whole> {
    let (mut workings, mut runs, mut sounds, mut all) = (0u64, 0u64, 0u64, 0u64);
    for (name, _) in f.files() {
        let ext = extension(name);
        workings += WORKINGS.contains(&ext.as_str()) as u64;
        runs += RUNS.contains(&ext.as_str()) as u64;
        sounds += (index::classify(Path::new(name)) == Some(What::Audio)) as u64;
        all += 1;
    }
    let made_of_them = all <= 100 || (workings + runs) * 20 >= all;
    if made_of_them && ((workings >= 1 && runs >= 1) || workings >= 5) {
        Some(Whole::Program)
    } else if sounds > HEAP {
        Some(Whole::Sounds)
    } else {
        None
    }
}

/// Adds up a folder and everything under it.
fn weigh(f: &Folder, into: &mut Program) {
    for (name, size) in f.files() {
        into.files += 1;
        into.bytes += size;
        match index::classify(Path::new(name)) {
            Some(What::Audio) => into.sounds += 1,
            Some(What::Photo) => into.pictures += 1,
            Some(What::Video) => into.videos += 1,
            Some(What::Document) => into.documents += 1,
            None => {}
        }
    }
    for inside in &f.folders {
        weigh(inside, into);
    }
}

struct Look<'a> {
    /// Folders that are a person's own whatever lies in them: the ones
    /// they chose to have read, and their home and its usual folders.
    own: &'a [PathBuf],
    files: Vec<(bool, PathBuf)>,
    programs: Vec<Program>,
}

impl Look<'_> {
    fn through(&mut self, f: &Folder, path: &mut PathBuf) {
        for (name, size) in f.files() {
            if let Some(what) = index::classify(Path::new(name)).filter(|what| index::fits_by(*what, name, size)) {
                self.files.push((what != What::Document, path.join(name)));
            }
        }
        for inside in &f.folders {
            if inside.denied || inside.hidden || index::passed_over(&inside.name) {
                continue;
            }
            path.push(&*inside.name);
            if let Some(what) = whole(inside).filter(|_| !self.own.iter().any(|own| own == path)) {
                let mut program = Program { path: path.clone(), what, ..Program::default() };
                weigh(inside, &mut program);
                self.programs.push(program);
            } else {
                self.through(inside, path);
            }
            path.pop();
        }
    }
}

/// The index that covers `root`, no older than [`FRESH_FOR`]: the kept
/// one if it is fresh, brought up to date if it is not, and made if
/// there is none, which is how it is when NeoShell has not run.
fn index_of(cache: &Path, root: &Path) -> Index {
    let old = |index: &Index| SystemTime::now().duration_since(UNIX_EPOCH).is_ok_and(|now| now.as_secs().saturating_sub(index.taken) > FRESH_FOR.as_secs());
    match keep::known(cache, root) {
        Some(index) if !old(&index) => index,
        // Of the disk it is on, where that is what was kept; of the folder itself otherwise.
        Some(index) => keep::up_to_date(cache, &index.root.clone(), &Progress::default()).0,
        None => keep::up_to_date(cache, root, &Progress::default()).0,
    }
}

/// Looks through the kept indexes for what is under `roots`. `cache` is
/// where indexes are kept.
pub fn survey(roots: &[PathBuf], cache: &Path) -> Survey {
    let home = neo_desktop::fs::home_dir();
    let mut own: Vec<PathBuf> = roots.to_vec();
    own.push(home);
    own.extend(["DESKTOP", "DOCUMENTS", "DOWNLOAD", "PICTURES", "MUSIC", "VIDEOS"].into_iter().map(neo_desktop::fs::user_dir));
    let mut look = Look { own: &own, files: vec![], programs: vec![] };
    for root in roots {
        let index = index_of(cache, root);
        // Paths are kept as the root was given, not as the index spells it.
        if let Some(folder) = index.folder_at(&keep::resolved(root)) {
            look.through(folder, &mut root.clone());
        }
    }
    // Documents first, which are quick; the rest keep the order they were found in.
    look.files.sort_by(|a, b| a.0.cmp(&b.0));
    look.files.dedup_by(|a, b| a.1 == b.1);
    look.programs.sort_by(|a, b| a.path.cmp(&b.path));
    look.programs.dedup_by(|a, b| a.path == b.path);
    Survey { files: look.files.into_iter().map(|f| f.1).collect(), programs: look.programs }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neo-apollo-survey-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![7u8; bytes]).unwrap();
    }

    #[test]
    fn a_disk_is_looked_through_by_its_index_and_programs_are_not_gone_into() {
        let dir = scratch("programs");
        let (root, cache) = (dir.join("disk"), dir.join("cache"));
        file(&root.join("Notes/plan.md"), 400);
        file(&root.join("Trip/IMG_1.jpg"), 20_000);
        file(&root.join("Trip/thumb.jpg"), 900);
        file(&root.join("Trip/.hidden/secret.md"), 50);
        file(&root.join("Trip/node_modules/readme.md"), 50);
        // A game: a program to run, its workings beside it, and thousands of sounds below.
        file(&root.join("Games/Racer/racer.exe"), 3_000);
        file(&root.join("Games/Racer/engine.dll"), 5_000);
        for i in 0..40 {
            file(&root.join(format!("Games/Racer/audio/vo/line{i}.ogg")), 9_000);
        }
        file(&root.join("Games/Racer/docs/manual.html"), 2_000);
        file(&root.join("Games/Racer/art/splash.png"), 30_000);
        // A heap of sounds, as a game or a recovery tool leaves: known whole too.
        for i in 0..(HEAP + 5) {
            file(&root.join(format!("Recovered/sounds/f{i}.wav")), 9_000);
        }
        file(&root.join("Recovered/notes.txt"), 300);
        // An installer among downloads does not make them a program's.
        file(&root.join("Downloads/setup.exe"), 4_000);
        file(&root.join("Downloads/invoice.pdf"), 6_000);

        let found = survey(std::slice::from_ref(&root), &cache);
        let mut names: Vec<String> = found.files.iter().map(|f| f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names.iter().take(2).filter(|n| n.ends_with(".md") || n.ends_with(".pdf")).count(), 2, "documents come first: {names:?}");
        names.sort();
        assert_eq!(names, ["Downloads/invoice.pdf", "Notes/plan.md", "Recovered/notes.txt", "Trip/IMG_1.jpg"], "not the icon, the hidden folder, the dependencies, the heap of sounds, or anything of the game's");
        assert_eq!(found.programs.iter().map(|p| (p.what, p.sounds)).collect::<Vec<_>>(), [(Whole::Program, 40), (Whole::Sounds, HEAP + 5)]);
        let game = &found.programs[0];
        assert_eq!((game.path.clone(), game.files, game.sounds, game.pictures, game.documents), (root.join("Games/Racer"), 44, 40, 1, 1));
        assert_eq!(game.bytes, 3_000 + 5_000 + 40 * 9_000 + 2_000 + 30_000);

        // A folder that was chosen to be read is read, whatever lies in it.
        let chosen = root.join("Games/Racer");
        let read = survey(std::slice::from_ref(&chosen), &cache);
        assert_eq!((read.programs.len(), read.files.len()), (0, 42), "the manual, the splash and the forty lines");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// By hand: looks through this computer's own disks, into an index
    /// kept in a folder of its own, and says what it found.
    /// `cargo test --release -p neo-apollo-core this_computer_is_looked -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn this_computer_is_looked_through() {
        let cache = scratch("real");
        let roots = crate::settings::fixed_drives();
        let roots = if roots.is_empty() { vec![neo_desktop::fs::home_dir()] } else { roots };
        let began = std::time::Instant::now();
        let found = survey(&roots, &cache);
        println!("first look, making the indexes: {:.1?}", began.elapsed());
        let began = std::time::Instant::now();
        let again = survey(&roots, &cache);
        println!("second look, from the kept indexes: {:.1?}", began.elapsed());
        assert_eq!(found.files.len(), again.files.len());
        let mut kinds = std::collections::BTreeMap::new();
        for f in &found.files {
            *kinds.entry(format!("{:?}", index::classify(f))).or_insert(0usize) += 1;
        }
        println!("to read: {} files {kinds:?}", found.files.len());
        let inside: u64 = found.programs.iter().map(|p| p.sounds + p.pictures + p.videos + p.documents).sum();
        println!("known whole: {} program folders, holding {inside} files that would have been read ({} sounds)", found.programs.len(), found.programs.iter().map(|p| p.sounds).sum::<u64>());
        let mut largest: Vec<&Program> = found.programs.iter().collect();
        largest.sort_by_key(|p| std::cmp::Reverse(p.sounds + p.pictures + p.videos + p.documents));
        for p in largest.iter().take(12) {
            println!("  {:>8} of them in {}", p.sounds + p.pictures + p.videos + p.documents, p.path.display());
        }
        let _ = std::fs::remove_dir_all(&cache);
    }
}