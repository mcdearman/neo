//! Reading picture files into [`Image`]s.

use std::path::Path;
use std::time::Duration;

use neo::Image;

/// The longest side kept. Larger pictures are scaled down as they load,
/// which keeps memory in check for panoramas and scans.
const MAX_SIDE: u32 = 8192;

/// The most memory an animation's frames may take. Longer animations play
/// as far as fits, then loop.
const MAX_ANIMATION_BYTES: usize = 384 * 1024 * 1024;

/// One frame of an animation and how long it stays on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub image: Image,
    pub delay: Duration,
}

/// A decoded picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    /// The picture, or the first frame of an animation.
    pub image: Image,
    /// The file's own size in pixels, before any scaling down.
    pub width: u32,
    pub height: u32,
    /// Every frame of an animated GIF or WebP. Empty for a still picture.
    pub frames: Vec<Frame>,
    /// Whether the animation was cut short to fit in memory.
    pub truncated: bool,
}

/// Reads the frames of an animation. Each one is the whole picture, already
/// composed. All frames share one texture, so playing costs one upload a frame.
fn animation<'a>(decoder: impl image::AnimationDecoder<'a>) -> Result<Decoded, String> {
    let mut frames: Vec<Frame> = Vec::new();
    let mut size = (0, 0);
    let mut bytes = 0;
    let mut truncated = false;
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|e| e.to_string())?;
        let (n, d) = frame.delay().numer_denom_ms();
        let ms = n.checked_div(d).unwrap_or(0);
        // Like browsers: a delay too short to show means "the default speed".
        let delay = Duration::from_millis(if ms < 20 { 100 } else { ms as u64 });
        let buffer = frame.into_buffer();
        let (w, h) = buffer.dimensions();
        bytes += w as usize * h as usize * 4;
        if bytes > MAX_ANIMATION_BYTES && !frames.is_empty() {
            truncated = true;
            break;
        }
        size = (w, h);
        let image = match frames.last() {
            Some(previous) => previous.image.next_frame(w, h, buffer.into_raw()),
            None => Image::frame(w, h, buffer.into_raw()),
        };
        frames.push(Frame { image, delay });
    }
    let first = frames.first().ok_or_else(|| "the file has no frames".to_string())?.image.clone();
    // One frame is just a picture.
    if frames.len() == 1 {
        frames.clear();
    }
    Ok(Decoded { image: first, width: size.0, height: size.1, frames, truncated })
}

/// Decodes with the `image` crate, turning the picture the way the camera
/// recorded it should be shown.
fn with_image_crate(path: &Path) -> Result<Decoded, String> {
    use image::{DynamicImage, ImageDecoder, ImageReader};
    let reader = ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    // GIFs and animated WebPs are read frame by frame, so they can play.
    let open = || std::fs::File::open(path).map(std::io::BufReader::new).map_err(|e| e.to_string());
    match reader.format() {
        Some(image::ImageFormat::Gif) => return animation(image::codecs::gif::GifDecoder::new(open()?).map_err(|e| e.to_string())?),
        Some(image::ImageFormat::WebP) => {
            let decoder = image::codecs::webp::WebPDecoder::new(open()?).map_err(|e| e.to_string())?;
            if decoder.has_animation() {
                return animation(decoder);
            }
        }
        _ => {}
    }
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut picture = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    picture.apply_orientation(orientation);
    let (width, height) = (picture.width(), picture.height());
    if width.max(height) > MAX_SIDE {
        picture = picture.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle);
    }
    let rgba = picture.into_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(Decoded { image: Image::new(w, h, rgba.into_raw()), width, height, frames: vec![], truncated: false })
}

/// Decodes with the system's ImageIO, for formats the `image` crate lacks.
#[cfg(target_os = "macos")]
fn with_system(path: &Path) -> Option<Decoded> {
    use std::ffi::{c_char, c_int, CString};
    unsafe extern "C" {
        fn neo_decode_image(path: *const c_char, max_side: c_int, width: *mut c_int, height: *mut c_int) -> *mut u8;
        fn neo_decode_free(pixels: *mut u8);
    }
    let c_path = CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let (mut w, mut h) = (0, 0);
    // SAFETY: the path is a valid C string and the out-pointers are valid.
    // The returned buffer holds w × h × 4 bytes and is freed exactly once.
    unsafe {
        let pixels = neo_decode_image(c_path.as_ptr(), MAX_SIDE as c_int, &mut w, &mut h);
        if pixels.is_null() || w <= 0 || h <= 0 {
            return None;
        }
        let data = std::slice::from_raw_parts(pixels, w as usize * h as usize * 4).to_vec();
        neo_decode_free(pixels);
        Some(Decoded { image: Image::new(w as u32, h as u32, data), width: w as u32, height: h as u32, frames: vec![], truncated: false })
    }
}

#[cfg(not(target_os = "macos"))]
fn with_system(_path: &Path) -> Option<Decoded> {
    None
}

/// Reads a picture. Takes a while for large files, so call it off the main thread.
pub fn decode(path: &Path) -> Result<Decoded, String> {
    match with_image_crate(path) {
        Ok(d) => Ok(d),
        // HEIC, AVIF and friends: let the system try before giving up.
        Err(e) => with_system(path).ok_or(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_png_and_reports_bad_files() {
        let dir = std::env::temp_dir().join(format!("neo-photos-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("dot.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255])).save(&png).unwrap();
        let d = decode(&png).unwrap();
        assert_eq!((d.width, d.height, d.image.width(), d.image.height()), (3, 2, 3, 2));
        let junk = dir.join("junk.png");
        std::fs::write(&junk, b"not a picture").unwrap();
        assert!(decode(&junk).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_every_frame_of_an_animated_gif() {
        use image::codecs::gif::{GifEncoder, Repeat};
        let dir = std::env::temp_dir().join(format!("neo-photos-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blink.gif");
        {
            let mut encoder = GifEncoder::new(std::fs::File::create(&path).unwrap());
            encoder.set_repeat(Repeat::Infinite).unwrap();
            for (shade, ms) in [(20u8, 80), (120, 200), (240, 10)] {
                let frame = image::Frame::from_parts(image::RgbaImage::from_pixel(12, 8, image::Rgba([shade, shade, shade, 255])), 0, 0, image::Delay::from_numer_denom_ms(ms, 1));
                encoder.encode_frame(frame).unwrap();
            }
        }
        let d = decode(&path).unwrap();
        assert_eq!((d.width, d.height, d.frames.len()), (12, 8, 3));
        let delays: Vec<u64> = d.frames.iter().map(|f| f.delay.as_millis() as u64).collect();
        assert_eq!(delays, [80, 200, 100], "a delay too short to show plays at the default speed");
        assert_eq!(d.image, d.frames[0].image);
        assert!(!d.truncated);

        // One frame is a still picture, with nothing to play.
        let still = dir.join("still.gif");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])).save(&still).unwrap();
        assert!(decode(&still).unwrap().frames.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// HEIC is what iPhones save. The `image` crate cannot read it, so this
    /// checks the system decoder takes over. `sips` ships with macOS.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_system_decodes_heic() {
        let dir = std::env::temp_dir().join(format!("neo-photos-heic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("in.png");
        image::RgbaImage::from_pixel(64, 48, image::Rgba([200, 40, 40, 255])).save(&png).unwrap();
        let heic = dir.join("out.heic");
        let made = std::process::Command::new("sips").args(["-s", "format", "heic"]).arg(&png).arg("--out").arg(&heic).output().is_ok_and(|o| o.status.success());
        if made {
            let d = decode(&heic).expect("HEIC decodes through ImageIO");
            assert_eq!((d.image.width(), d.image.height()), (64, 48));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
