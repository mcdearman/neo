//! Sight: what a picture is of, as numbers that can be held against
//! another picture's.
//!
//! Two things are taken from a picture. Of the whole of it, an embedding
//! (512 numbers, from CLIP's eye): two pictures of the same scene, or two
//! moments of one video, come out near each other, whatever anyone would
//! say of them in words. And of each face in it, an embedding of that
//! face (128 numbers, from SFace, found with YuNet): two pictures of one
//! person come out near each other wherever they were taken.
//!
//! Nothing here knows who anyone is; it only tells that two faces are
//! alike. The models run on this computer and nothing leaves it.

use std::path::{Path, PathBuf};

use image::RgbImage;
use ort::session::Session;
use ort::value::Tensor;

/// A model's file, where it is fetched from, and how to know it is whole.
pub struct ModelFile {
    pub file: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// The models sight needs: the whole picture's, the face finder, the face's.
pub const MODELS: [ModelFile; 3] = [
    ModelFile { file: "clip-vit-b32-vision.onnx", url: "https://huggingface.co/Qdrant/clip-ViT-B-32-vision/resolve/main/model.onnx", bytes: 351_700_000, sha256: "c68d3d9a200ddd2a8c8a5510b576d4c94d1ae383bf8b36dd8c084f94e1fb4d63" },
    ModelFile { file: "face-detect-yunet.onnx", url: "https://github.com/opencv/opencv_zoo/raw/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx", bytes: 233_000, sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4" },
    ModelFile { file: "face-sface.onnx", url: "https://github.com/opencv/opencv_zoo/raw/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx", bytes: 38_700_000, sha256: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79" },
];

/// Those of the models that are not in `dir` yet.
pub fn missing(dir: &Path) -> Vec<&'static ModelFile> {
    MODELS.iter().filter(|m| !dir.join(m.file).is_file()).collect()
}

/// How long the whole picture's embedding is, and a face's.
pub const SCENE: usize = 512;
pub const FACE: usize = 128;

/// Two faces nearer than this are taken for one person's. SFace's makers
/// give 0.363 as where its own trials divide; this is a little stricter,
/// since a wrong "the same person" is worse here than a missed one.
pub const SAME_FACE: f32 = 0.40;

/// A face found in a picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Where it is, in the picture's own pixels: left, top, width, height.
    pub rect: (f32, f32, f32, f32),
    /// How sure the finder is that it is a face, of 1.
    pub sure: f32,
    /// What tells it from other faces: [`FACE`] numbers, of length one.
    pub vector: Vec<f32>,
}

/// What was made of a picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    /// The whole picture: [`SCENE`] numbers, of length one.
    pub scene: Vec<f32>,
    pub faces: Vec<Face>,
}

/// How alike two embeddings of length one are: 1 the same, 0 nothing alike.
pub fn alike(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let length = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if length > 0.0 {
        for x in &mut v {
            *x /= length;
        }
    }
    v
}

fn wrong(e: impl std::fmt::Display) -> String {
    format!("The picture could not be looked at: {e}")
}

/// The models, loaded and ready to look.
pub struct Sight {
    scene: Session,
    finder: Session,
    face: Session,
    names: Names,
}

/// What each model calls what goes into it and what comes out.
struct Names {
    scene_in: String,
    scene_out: String,
    finder_in: String,
    face_in: String,
    face_out: String,
}

/// The side of the picture the face finder is shown, and the strides of
/// the three grids it answers on.
const FIND_SIDE: u32 = 640;
const STRIDES: [u32; 3] = [8, 16, 32];
/// How sure the finder must be, and how much two finds may overlap
/// before they are one face.
const SURE: f32 = 0.8;
const OVERLAP: f32 = 0.3;
/// A face smaller than this across, in the picture's own pixels, is too
/// little to tell from another.
const SMALLEST_FACE: f32 = 36.0;
/// The most faces taken from one picture: a crowd is not people one knows.
const MOST_FACES: usize = 12;

impl Sight {
    /// Loads the models from `dir`. `threads` is how many each may work on.
    pub fn open(dir: &Path, threads: usize) -> Result<Self, String> {
        let load = |file: &str| -> Result<Session, String> {
            let path: PathBuf = dir.join(file);
            Session::builder().map_err(wrong)?.with_intra_threads(threads.max(1)).map_err(wrong)?.commit_from_file(&path).map_err(|e| format!("The model {} could not be loaded: {e}", path.display()))
        };
        let (scene, finder, face) = (load(MODELS[0].file)?, load(MODELS[1].file)?, load(MODELS[2].file)?);
        let first_in = |s: &Session| s.inputs().first().map(|i| i.name().to_owned()).unwrap_or_default();
        // The whole picture's model gives more than one thing; the one wanted is the picture's own embedding.
        let scene_out = scene.outputs().iter().map(|o| o.name().to_owned()).find(|n| n.contains("embed")).or_else(|| scene.outputs().last().map(|o| o.name().to_owned())).unwrap_or_default();
        let names = Names { scene_in: first_in(&scene), scene_out, finder_in: first_in(&finder), face_in: first_in(&face), face_out: face.outputs().first().map(|o| o.name().to_owned()).unwrap_or_default() };
        Ok(Self { scene, finder, face, names })
    }

    /// What goes into and comes out of each model, for looking into by hand.
    pub fn described(&self) -> String {
        let of = |what: &str, s: &Session| format!("{what}: in {:?}, out {:?}", s.inputs().iter().map(|i| i.name()).collect::<Vec<_>>(), s.outputs().iter().map(|o| o.name()).collect::<Vec<_>>());
        [of("scene", &self.scene), of("finder", &self.finder), of("face", &self.face)].join("\n")
    }

    /// Looks at a picture: the whole of it, and each face in it.
    pub fn look(&mut self, picture: &RgbImage) -> Result<Seen, String> {
        Ok(Seen { scene: self.scene_of(picture)?, faces: self.faces_of(picture)? })
    }

    /// The whole picture's embedding.
    pub fn scene_of(&mut self, picture: &RgbImage) -> Result<Vec<f32>, String> {
        const SIDE: u32 = 224;
        const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
        const SPREAD: [f32; 3] = [0.268_629_54, 0.261_302_58, 0.275_777_11];
        if picture.width() == 0 || picture.height() == 0 {
            return Err(wrong("it is empty"));
        }
        // Its shorter side to 224, and the middle of it kept.
        let by = SIDE as f32 / picture.width().min(picture.height()) as f32;
        let (w, h) = (((picture.width() as f32 * by).round() as u32).max(SIDE), ((picture.height() as f32 * by).round() as u32).max(SIDE));
        let small = image::imageops::resize(picture, w, h, image::imageops::FilterType::CatmullRom);
        let (left, top) = ((w - SIDE) / 2, (h - SIDE) / 2);
        let mut data = vec![0.0f32; (3 * SIDE * SIDE) as usize];
        for y in 0..SIDE {
            for x in 0..SIDE {
                let p = small.get_pixel(left + x, top + y).0;
                for c in 0..3 {
                    data[(c as u32 * SIDE * SIDE + y * SIDE + x) as usize] = (p[c] as f32 / 255.0 - MEAN[c]) / SPREAD[c];
                }
            }
        }
        let input = Tensor::from_array(([1usize, 3, SIDE as usize, SIDE as usize], data)).map_err(wrong)?;
        let outputs = self.scene.run(ort::inputs![self.names.scene_in.as_str() => input]).map_err(wrong)?;
        let (_, numbers) = outputs[self.names.scene_out.as_str()].try_extract_tensor::<f32>().map_err(wrong)?;
        if numbers.len() < SCENE {
            return Err(wrong("the model answered with too little"));
        }
        Ok(unit(numbers[..SCENE].to_vec()))
    }

    /// The faces in a picture, the surest first, each with its embedding.
    pub fn faces_of(&mut self, picture: &RgbImage) -> Result<Vec<Face>, String> {
        let found = self.find(picture)?;
        let mut faces = Vec::with_capacity(found.len());
        for (rect, sure, marks) in found {
            let vector = self.face_of(picture, &marks)?;
            faces.push(Face { rect, sure, vector });
        }
        Ok(faces)
    }

    /// Where the faces are: each one's place, how sure, and its five
    /// marks (the eyes, the nose, the corners of the mouth).
    #[allow(clippy::type_complexity)]
    fn find(&mut self, picture: &RgbImage) -> Result<Vec<((f32, f32, f32, f32), f32, [(f32, f32); 5])>, String> {
        if picture.width() == 0 || picture.height() == 0 {
            return Ok(vec![]);
        }
        // Fitted into the finder's square from its top left corner, the rest left black.
        let by = FIND_SIDE as f32 / picture.width().max(picture.height()) as f32;
        let (w, h) = (((picture.width() as f32 * by).round() as u32).clamp(1, FIND_SIDE), ((picture.height() as f32 * by).round() as u32).clamp(1, FIND_SIDE));
        let small = image::imageops::resize(picture, w, h, image::imageops::FilterType::Triangle);
        let side = FIND_SIDE as usize;
        let mut data = vec![0.0f32; 3 * side * side];
        for (x, y, p) in small.enumerate_pixels() {
            // Blue, green, red, each as it is, of 255: what this finder was taught on.
            for c in 0..3 {
                data[c * side * side + y as usize * side + x as usize] = p.0[2 - c] as f32;
            }
        }
        let input = Tensor::from_array(([1usize, 3, side, side], data)).map_err(wrong)?;
        let outputs = self.finder.run(ort::inputs![self.names.finder_in.as_str() => input]).map_err(wrong)?;
        let mut found: Vec<((f32, f32, f32, f32), f32, [(f32, f32); 5])> = vec![];
        for stride in STRIDES {
            let get = |what: &str| -> Result<Vec<f32>, String> { Ok(outputs[format!("{what}_{stride}").as_str()].try_extract_tensor::<f32>().map_err(wrong)?.1.to_vec()) };
            let (cls, obj, bbox, kps) = (get("cls")?, get("obj")?, get("bbox")?, get("kps")?);
            let (cols, rows) = ((FIND_SIDE / stride) as usize, (FIND_SIDE / stride) as usize);
            let s = stride as f32;
            for r in 0..rows {
                for c in 0..cols {
                    let i = r * cols + c;
                    let (Some(cl), Some(ob)) = (cls.get(i), obj.get(i)) else { continue };
                    let sure = (cl.clamp(0.0, 1.0) * ob.clamp(0.0, 1.0)).sqrt();
                    if sure < SURE || bbox.len() < i * 4 + 4 || kps.len() < i * 10 + 10 {
                        continue;
                    }
                    let (cx, cy) = ((c as f32 + bbox[i * 4]) * s, (r as f32 + bbox[i * 4 + 1]) * s);
                    let (bw, bh) = (bbox[i * 4 + 2].exp() * s, bbox[i * 4 + 3].exp() * s);
                    let mut marks = [(0.0, 0.0); 5];
                    for (k, mark) in marks.iter_mut().enumerate() {
                        *mark = ((c as f32 + kps[i * 10 + k * 2]) * s / by, (r as f32 + kps[i * 10 + k * 2 + 1]) * s / by);
                    }
                    found.push((((cx - bw / 2.0) / by, (cy - bh / 2.0) / by, bw / by, bh / by), sure, marks));
                }
            }
        }
        // The surest first; one that mostly covers a surer one is the same face again.
        found.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut kept: Vec<((f32, f32, f32, f32), f32, [(f32, f32); 5])> = vec![];
        for face in found {
            let (x, y, w, h) = face.0;
            if w.min(h) < SMALLEST_FACE {
                continue;
            }
            let covers = |other: &(f32, f32, f32, f32)| {
                let (ox, oy, ow, oh) = *other;
                let across = ((x + w).min(ox + ow) - x.max(ox)).max(0.0) * ((y + h).min(oy + oh) - y.max(oy)).max(0.0);
                across / (w * h + ow * oh - across).max(f32::MIN_POSITIVE)
            };
            if kept.iter().all(|k| covers(&k.0) < OVERLAP) {
                kept.push(face);
                if kept.len() >= MOST_FACES {
                    break;
                }
            }
        }
        Ok(kept)
    }

    /// A face's embedding: the face is first set straight, its eyes, nose
    /// and mouth brought to where the model expects them.
    fn face_of(&mut self, picture: &RgbImage, marks: &[(f32, f32); 5]) -> Result<Vec<f32>, String> {
        const SIDE: usize = 112;
        // Where the five marks belong in a face 112 across.
        const PLACES: [(f32, f32); 5] = [(38.2946, 51.6963), (73.5318, 51.5014), (56.0252, 71.7366), (41.5493, 92.3655), (70.7299, 92.2041)];
        // The turn, size and shift that best carry the marks to their places.
        let mean = |points: &[(f32, f32); 5]| (points.iter().map(|p| p.0).sum::<f32>() / 5.0, points.iter().map(|p| p.1).sum::<f32>() / 5.0);
        let (from, to) = (mean(marks), mean(&PLACES));
        let (mut dot, mut cross, mut spread) = (0.0f32, 0.0f32, 0.0f32);
        for (p, q) in marks.iter().zip(&PLACES) {
            let (px, py, qx, qy) = (p.0 - from.0, p.1 - from.1, q.0 - to.0, q.1 - to.1);
            dot += px * qx + py * qy;
            cross += px * qy - py * qx;
            spread += px * px + py * py;
        }
        if spread <= 0.0 {
            return Err(wrong("a face's marks are all in one place"));
        }
        let (a, b) = (dot / spread, cross / spread);
        let size = (a * a + b * b).max(f32::MIN_POSITIVE);
        let (w, h) = (picture.width() as i64, picture.height() as i64);
        let mut data = vec![0.0f32; 3 * SIDE * SIDE];
        for v in 0..SIDE {
            for u in 0..SIDE {
                // Back from the straightened face to where that is in the picture.
                let (du, dv) = (u as f32 - to.0, v as f32 - to.1);
                let (x, y) = ((a * du + b * dv) / size + from.0, (-b * du + a * dv) / size + from.1);
                let (x0, y0) = (x.floor(), y.floor());
                let (fx, fy) = (x - x0, y - y0);
                let at = |xi: i64, yi: i64, c: usize| -> f32 { if xi < 0 || yi < 0 || xi >= w || yi >= h { 0.0 } else { picture.get_pixel(xi as u32, yi as u32).0[c] as f32 } };
                for c in 0..3 {
                    let (xi, yi) = (x0 as i64, y0 as i64);
                    let value = at(xi, yi, c) * (1.0 - fx) * (1.0 - fy) + at(xi + 1, yi, c) * fx * (1.0 - fy) + at(xi, yi + 1, c) * (1.0 - fx) * fy + at(xi + 1, yi + 1, c) * fx * fy;
                    // Red, green, blue, each of 255.
                    data[c * SIDE * SIDE + v * SIDE + u] = value;
                }
            }
        }
        let input = Tensor::from_array(([1usize, 3, SIDE, SIDE], data)).map_err(wrong)?;
        let outputs = self.face.run(ort::inputs![self.names.face_in.as_str() => input]).map_err(wrong)?;
        let (_, numbers) = outputs[self.names.face_out.as_str()].try_extract_tensor::<f32>().map_err(wrong)?;
        if numbers.len() < FACE {
            return Err(wrong("the model answered with too little"));
        }
        Ok(unit(numbers[..FACE].to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeddings_of_length_one_are_alike_as_they_point_the_same_way() {
        let a = unit(vec![3.0, 4.0]);
        assert!((alike(&a, &a) - 1.0).abs() < 1e-6 && alike(&a, &unit(vec![-4.0, 3.0])).abs() < 1e-6);
        assert_eq!(unit(vec![0.0, 0.0]), [0.0, 0.0]);
    }

    /// By hand, with the models fetched and real pictures:
    /// `NEO_SIGHT_MODELS=dir NEO_SIGHT_PICTURES="a.jpg;b.jpg" cargo test --release -p neo-sight real_pictures -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_pictures_are_looked_at() {
        let dir = PathBuf::from(std::env::var_os("NEO_SIGHT_MODELS").expect("say where the models are"));
        let began = std::time::Instant::now();
        let mut sight = Sight::open(&dir, 8).unwrap();
        println!("{}\nloaded in {:.1?}", sight.described(), began.elapsed());
        let mut seen: Vec<(String, Seen)> = vec![];
        for file in std::env::var("NEO_SIGHT_PICTURES").unwrap_or_default().split(';').filter(|f| !f.is_empty()) {
            let picture = image::ImageReader::open(file).unwrap().with_guessed_format().unwrap().decode().unwrap().to_rgb8();
            let began = std::time::Instant::now();
            let s = sight.look(&picture).unwrap();
            let name = Path::new(file).file_name().unwrap().to_string_lossy().into_owned();
            println!("{name}: {}x{}, {} faces {:?} in {:.0?}", picture.width(), picture.height(), s.faces.len(), s.faces.iter().map(|f| (f.rect.0 as i32, f.rect.1 as i32, f.rect.2 as i32, f.rect.3 as i32, (f.sure * 100.0) as i32)).collect::<Vec<_>>(), began.elapsed());
            // The same picture made smaller is still that picture, and that face.
            let half = image::imageops::resize(&picture, picture.width() / 2, picture.height() / 2, image::imageops::FilterType::Triangle);
            let again = sight.look(&half).unwrap();
            println!("  half the size: scene {:.3} alike; faces {:?}", alike(&s.scene, &again.scene), s.faces.iter().zip(&again.faces).map(|(a, b)| format!("{:.3}", alike(&a.vector, &b.vector))).collect::<Vec<_>>());
            seen.push((name, s));
        }
        for (i, (a, sa)) in seen.iter().enumerate() {
            for (b, sb) in seen.iter().skip(i + 1) {
                let faces = sa.faces.iter().flat_map(|fa| sb.faces.iter().map(move |fb| alike(&fa.vector, &fb.vector))).fold(f32::MIN, f32::max);
                println!("{a} and {b}: scene {:.3}, nearest faces {}", alike(&sa.scene, &sb.scene), if faces == f32::MIN { "none".to_owned() } else { format!("{faces:.3}") });
            }
        }
    }
}