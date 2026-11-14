//! Telling pictures and people apart by looking, not by what was said of
//! them.
//!
//! Every photo, and several moments of every video, is given to
//! [`neo_sight`]: the whole picture comes back as an embedding, and so
//! does each face in it. They are kept in the memory, encrypted with the
//! rest, a byte a number. A picture someone pastes into the search is
//! looked at the same way and held against them all: the video it was
//! taken from, and other pictures of the same place, come out by the
//! whole picture; other pictures of the same person, wherever they were
//! taken, by the face.
//!
//! Nothing is named. A face's embedding says only that two faces are
//! alike. It is of people all the same, and is kept nowhere but in the
//! memory, and goes when the file is forgotten.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock, mpsc};

use image::RgbImage;
use neo_sight::Sight;

use crate::index::{self, Progress, What};
use crate::store::Store;

/// Where the models are kept, once it has been said.
static MODELS: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Has pictures looked at with the models kept in `dir`, fetching them
/// there the first time. The app says this once as it starts; a test does
/// not, and then nothing is looked at this way and nothing is fetched.
pub fn use_models(dir: PathBuf) {
    if let Ok(mut at) = MODELS.write() {
        *at = Some(dir);
    }
}

/// Where the models are, if looking this way has been asked for.
pub fn models() -> Option<PathBuf> {
    MODELS.read().ok().and_then(|dir| dir.clone())
}

/// Where the models are kept on this system: with what Neo has fetched,
/// not with what the user has made.
pub fn models_dir() -> PathBuf {
    let home = neo_desktop::fs::home_dir();
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData").join("Local")).join("Neo").join("models").join("sight")
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Neo/models/sight")
    } else {
        std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("neo/models/sight")
    }
}

/// Fetches whichever of the models are not in `dir` yet, some 370 MB the
/// first time. `progress` hears how many bytes of how many; false gives up.
pub fn fetch(dir: &Path, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<(), String> {
    for model in neo_sight::missing(dir) {
        crate::hear::download_to(model.url, &dir.join(model.file), model.bytes, progress)?;
    }
    Ok(())
}

/// The models, ready, with as many threads as leaves the computer usable.
pub fn open(dir: &Path) -> Result<Sight, String> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get() / 2).clamp(2, 12);
    Sight::open(dir, threads)
}

/// A picture given as its pixels (RGBA, top row first), as sight is shown
/// it: one that is in no file, such as one copied from a page.
pub fn pixels(width: u32, height: u32, rgba: &[u8]) -> Option<RgbImage> {
    let whole = image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(width, height, rgba.to_vec())?);
    Some(if width.max(height) > LONGEST { whole.thumbnail(LONGEST, LONGEST) } else { whole }.to_rgb8())
}

/// What sight makes of one picture, for holding against what it has
/// kept: the whole of it, and each face in it. The models are loaded for
/// it and let go after, which is under a second.
pub fn seen_in(picture: &RgbImage) -> Result<(Vec<f32>, Vec<Vec<f32>>), String> {
    let dir = models().ok_or("Looking at pictures is not set up here.")?;
    if !neo_sight::missing(&dir).is_empty() {
        return Err("The models that tell pictures apart have not been fetched yet; they are when Apollo next reads.".into());
    }
    let seen = open(&dir)?.look(picture)?;
    Ok((seen.scene, seen.faces.into_iter().map(|f| f.vector).collect()))
}

/// Added to a file's stamp once sight has taken it in. Changing it has
/// everything looked at again, which is what a better model would want.
const STAMP: &str = "-s1";
/// How many files are opened and made ready at once. Looking itself is
/// one at a time, on many threads.
const READERS: usize = 6;
/// A picture is made no larger than this on its longest side before it
/// is looked at: enough for a face across a room, and quick.
const LONGEST: u32 = 1280;

/// The moments of a video sight looks at, in seconds: more than are
/// described in words, since looking is quick, and a frame someone pastes
/// is then seldom far from one of them. One for every quarter minute,
/// between two and ten, spread evenly; one for a clip of a few seconds.
pub fn moments(seconds: f64) -> Vec<f64> {
    let many = if seconds < 3.0 { 1 } else { ((seconds / 15.0).ceil() as usize).clamp(2, 10) };
    (0..many).map(|i| (seconds * (i as f64 + 0.5) / many as f64 * 10.0).round() / 10.0).collect()
}

/// A picture file as sight is shown it.
pub fn picture(path: &Path) -> Result<RgbImage, String> {
    let read = image::ImageReader::open(path).map_err(|e| e.to_string()).and_then(|r| r.with_guessed_format().map_err(|e| e.to_string())).and_then(|r| r.decode().map_err(|e| e.to_string()));
    let whole = match read {
        Ok(whole) => whole,
        // A kind this cannot read itself: by way of whatever can.
        Err(_) => image::load_from_memory(&index::shown_from(path)?).map_err(|e| e.to_string())?,
    };
    Ok(if whole.width().max(whole.height()) > LONGEST { whole.thumbnail(LONGEST, LONGEST) } else { whole }.to_rgb8())
}

/// What there is to look at in a file: a photo itself, or the moments of
/// a video, each with when it is.
fn frames(path: &Path, what: What) -> Result<Vec<(f32, RgbImage)>, String> {
    match what {
        What::Video => {
            let seconds = index::duration(path).ok_or("The video's length could not be read.")?;
            let found: Vec<(f32, RgbImage)> = moments(seconds).into_iter().filter_map(|at| Some((at as f32, image::load_from_memory(&index::frame(path, at)?).ok()?.to_rgb8()))).collect();
            if found.is_empty() { Err("No moment of the video could be read.".into()) } else { Ok(found) }
        }
        _ => Ok(vec![(0.0, picture(path)?)]),
    }
}

/// What sight makes of each of some pictures: when it is, the whole of
/// it, and its faces.
type Looked = Vec<(f32, Vec<f32>, Vec<Vec<f32>>)>;

fn look(sight: &Mutex<Sight>, frames: &[(f32, RgbImage)]) -> Result<Looked, String> {
    let mut sight = sight.lock().map_err(|_| "Sight is in a bad way.".to_owned())?;
    frames
        .iter()
        .map(|(at, picture)| {
            let seen = sight.look(picture)?;
            Ok((*at, seen.scene, seen.faces.into_iter().map(|f| f.vector).collect()))
        })
        .collect()
}

/// Looks at every photo and video among `files` that has not been looked
/// at since it last changed, and keeps what was seen. `report` hears of
/// each file as it is done; returning false stops there, to be taken up
/// again later. Does nothing where no models have been named.
pub fn pass(store: &mut Store, files: &[PathBuf], mut p: Progress, report: &mut dyn FnMut(&Progress) -> bool) -> Progress {
    let Some(dir) = models() else { return p };
    let (done_before, total_before) = (p.done, p.total);
    // What there is to do is settled first: most files are as they were.
    let mut waiting: Vec<(PathBuf, What, String, u64)> = vec![];
    for file in files {
        let Some(what @ (What::Photo | What::Video)) = index::classify(file) else { continue };
        let Ok(meta) = std::fs::metadata(file) else { continue };
        let stamp = format!("{}{STAMP}", index::stamp(&meta));
        if store.sighted_stamp(file).ok().flatten().as_deref() != Some(stamp.as_str()) {
            waiting.push((file.clone(), what, stamp, index::modified(&meta)));
        }
    }
    if waiting.is_empty() {
        return p;
    }
    (p.done, p.total, p.sighting, p.looking) = (0, waiting.len(), true, false);
    let finish = |mut p: Progress| {
        (p.done, p.total, p.sighting, p.now) = (done_before, total_before, false, None);
        p
    };
    if !report(&p) {
        return finish(p);
    }
    // The models, fetched the first time: a third of a gigabyte.
    if !neo_sight::missing(&dir).is_empty()
        && let Err(e) = fetch(&dir, &mut |_, _| report(&p))
    {
        p.trouble = Some(e);
        return finish(p);
    }
    let sight = match open(&dir) {
        Ok(sight) => Mutex::new(sight),
        Err(e) => {
            p.trouble = Some(e);
            return finish(p);
        }
    };
    // A little of every folder first, the newest of each, as with reading.
    let waiting = index::spread(waiting, |w| (w.0.parent().map(Path::to_path_buf).unwrap_or_default(), w.3));
    let queue: Mutex<VecDeque<(PathBuf, What, String, u64)>> = Mutex::new(waiting.into());
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let (made, taken) = mpsc::sync_channel::<(PathBuf, String, Result<Looked, String>)>(64);
        for _ in 0..READERS {
            let (made, queue, stop, sight) = (made.clone(), &queue, &stop, &sight);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    // A question comes first.
                    if index::being_asked() {
                        std::thread::sleep(std::time::Duration::from_millis(25));
                        continue;
                    }
                    let Some((file, what, stamp, _)) = queue.lock().ok().and_then(|mut q| q.pop_front()) else { break };
                    let looked = frames(&file, what).and_then(|frames| look(sight, &frames));
                    if made.send((file, stamp, looked)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(made);
        // Kept by this one hand, some at a time.
        let mut since = std::time::Instant::now();
        let mut open = false;
        while let Ok((file, stamp, looked)) = taken.recv() {
            if !open {
                open = store.begin().is_ok();
            }
            // One that could not be looked at is noted all the same, so
            // that it is not tried again every time; it is when it changes.
            let kept = match looked {
                Ok(looked) => store.set_sights(&file, &stamp, &looked),
                Err(_) => store.set_sights(&file, &stamp, &[]),
            };
            if let Err(e) = kept {
                p.trouble = Some(e);
            }
            p.done += 1;
            p.now = Some(file);
            if open && (since.elapsed() >= std::time::Duration::from_millis(800) || p.done % 200 == 0) {
                let _ = store.commit();
                (open, since) = (false, std::time::Instant::now());
            }
            if !report(&p) {
                stop.store(true, Ordering::Relaxed);
            }
        }
        if open {
            let _ = store.commit();
        }
    });
    finish(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_is_looked_at_at_moments_spread_along_it() {
        assert_eq!(moments(2.0), [1.0]);
        assert_eq!(moments(20.0), [5.0, 15.0]);
        assert_eq!(moments(60.0).len(), 4);
        let long = moments(3600.0);
        assert!(long.len() == 10 && long[0] == 180.0 && long[9] == 3420.0, "{long:?}");
    }

    /// By hand, on this computer's own pictures and models:
    /// `NEO_SIGHT_FOLDER=dir cargo test --release -p neo-apollo-core real_folder -- --ignored --nocapture`
    /// Looks at what is in the folder, keeps it in a memory of its own,
    /// and says what each file is most like.
    #[test]
    #[ignore]
    fn a_real_folder_is_looked_at_and_searched() {
        let folder = PathBuf::from(std::env::var_os("NEO_SIGHT_FOLDER").expect("say which folder"));
        use_models(models_dir());
        let db = std::env::temp_dir().join(format!("neo-apollo-sight-real-{}", std::process::id())).join("memory.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let mut store = Store::open(&db, &[7; 32], 8).unwrap();
        let files = index::walk(&[folder]);
        let began = std::time::Instant::now();
        let p = pass(&mut store, &files, Progress::default(), &mut |_| true);
        let (seen, faces) = store.sight_counts().unwrap();
        println!("{} files looked at in {:.1?} ({:.0} ms each), {faces} faces; trouble: {:?}", seen, began.elapsed(), began.elapsed().as_secs_f64() * 1000.0 / seen.max(1) as f64, p.trouble);
        let sight = Mutex::new(open(&models_dir()).unwrap());
        let mut shown = 0;
        for file in files.iter().filter(|f| index::classify(f) == Some(What::Photo)) {
            if shown >= 10 { break; }
            let Ok(frames) = frames(file, What::Photo) else { continue };
            let looked = look(&sight, &frames).unwrap();
            // Those with a face in them are what is wanted here.
            if looked[0].2.is_empty() { continue; }
            shown += 1;
            let began = std::time::Instant::now();
            let found = store.search_sights(None, &looked[0].2, 5).unwrap();
            println!("{} ({} faces), searched in {:.0?}:", file.file_name().unwrap().to_string_lossy(), looked[0].2.len(), began.elapsed());
            for m in found {
                println!("    {:>3.0}%  whole {:.2}  face {:.2}  {}", m.score() * 100.0, m.scene, m.face, m.source.file_name().unwrap().to_string_lossy());
            }
        }
        drop(store);
        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }
}