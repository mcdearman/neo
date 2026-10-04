//! Draws the app icons with `neo-render` into `dist/icons`.
//!
//! Two shapes: a full-bleed rounded square for Linux and Windows, and the
//! macOS layout, which leaves a margin and a shadow around the tile.

use std::path::Path;

use neo_render::{FontFamily, Point, Rect, Renderer, Scene, TextStyle};
use neo_theme::{Color, Shadow};

use crate::apps::{AppInfo, APPS};

/// Sizes for the freedesktop.org hicolor theme and Windows .ico files.
pub const SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

/// Draws one icon on a 1024-unit canvas.
fn scene(r: &mut Renderer, app: &AppInfo, macos: bool) -> Scene {
    let mut scene = Scene::new(Color::TRANSPARENT);
    // macOS app icons sit in an 824-unit tile; other platforms fill more of the canvas.
    let (inset, radius) = if macos { (100.0, 185.0) } else { (48.0, 230.0) };
    let tile = Rect::new(inset, inset, 1024.0 - inset * 2.0, 1024.0 - inset * 2.0);
    if macos {
        let shadow = Shadow { offset: (0.0, 10.0), blur: 28.0, spread: 0.0, color: Color::BLACK.with_alpha(0.3), inset: false };
        scene.shadow(tile, radius, &shadow);
    }
    scene.gradient(tile, radius, app.top, Point::new(512.0, tile.y), app.bottom, Point::new(512.0, tile.bottom()));
    // A faint rim keeps dark tiles distinct on dark docks.
    scene.fill(tile, radius, Color::TRANSPARENT, Some((4.0, Color::WHITE.with_alpha(0.12))));
    let size = tile.w * 0.54;
    let glyph = r.text().layout(&app.glyph.0.to_string(), &TextStyle { size, family: FontFamily::Icons, line_height: 1.0, ..Default::default() }, None);
    let s = glyph.size();
    let c = tile.center();
    scene.text(&glyph, Point::new(c.x - s.w * 0.5, c.y - s.h * 0.5), app.ink);
    scene
}

fn save(r: &mut Renderer, scene: &Scene, px: u32, path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    r.save_png(scene, px, px, px as f32 / 1024.0, path)
}

/// Icons for the Recorder's place in the menu bar or system tray: a
/// template glyph that the system tints, and a red dot shown while recording.
fn tray(r: &mut Renderer, out: &Path) -> std::io::Result<()> {
    let mut idle = Scene::new(Color::TRANSPARENT);
    let glyph = r.text().layout(&neo_theme::icons::VIDEO.0.to_string(), &TextStyle { size: 800.0, family: FontFamily::Icons, line_height: 1.0, ..Default::default() }, None);
    let s = glyph.size();
    idle.text(&glyph, Point::new(512.0 - s.w * 0.5, 512.0 - s.h * 0.5), Color::BLACK);
    save(r, &idle, 44, &out.join("tray/recorder.png"))?;

    let mut recording = Scene::new(Color::TRANSPARENT);
    recording.fill(Rect::new(192.0, 192.0, 640.0, 640.0), 320.0, Color::hex(0xFF3B30), None);
    save(r, &recording, 44, &out.join("tray/recorder-recording.png"))
}

/// Writes `hicolor/NxN/apps/<id>.png` and `macos/<id>.iconset/*.png` under `out`.
pub fn render_all(out: &Path) -> Result<(), String> {
    let mut r = Renderer::headless()?;
    tray(&mut r, out).map_err(|e| e.to_string())?;
    for app in APPS {
        let flat = scene(&mut r, app, false);
        for px in SIZES {
            save(&mut r, &flat, px, &out.join(format!("hicolor/{px}x{px}/apps/{}.png", app.id))).map_err(|e| e.to_string())?;
        }
        let mac = scene(&mut r, app, true);
        for base in [16, 32, 128, 256, 512] {
            let set = out.join(format!("macos/{}.iconset", app.id));
            save(&mut r, &mac, base, &set.join(format!("icon_{base}x{base}.png"))).map_err(|e| e.to_string())?;
            save(&mut r, &mac, base * 2, &set.join(format!("icon_{base}x{base}@2x.png"))).map_err(|e| e.to_string())?;
        }
        println!("drew {}", app.id);
    }
    Ok(())
}
