//! Renders every primitive to `target/primitives.png`.

use armature_render::{FontFamily, Point, Rect, Renderer, Scene, TextStyle};
use neo_theme::{Color, Scheme, Surface, Theme};

fn main() {
    let mut r = Renderer::headless(neo_theme::fonts::bundled()).expect("GPU");
    let (w, h, scale) = (900.0, 520.0, 2.0);
    let mut scene = Scene::new(Color::TRANSPARENT);

    for (col, scheme) in [Scheme::Light, Scheme::Dark].into_iter().enumerate() {
        let theme = Theme { scheme, ..Theme::default() };
        let p = theme.palette();
        let x0 = col as f32 * 450.0;
        let panel = Rect::new(x0, 0.0, 450.0, h);
        scene.fill(panel, 0.0, p.bg, None);

        let title = r.text().layout(if col == 0 { "Light" } else { "Dark" }, &TextStyle { size: 22.0, weight: 800, ..Default::default() }, None);
        scene.text(&title, Point::new(x0 + 30.0, 24.0), p.text);

        let surfaces = [(Surface::Card, "Card"), (Surface::Raised, "Raised"), (Surface::Pressed, "Pressed"), (Surface::Well, "Well"), (Surface::Accent, "Accent")];
        for (i, (s, name)) in surfaces.iter().enumerate() {
            let rect = Rect::new(x0 + 30.0 + (i % 3) as f32 * 135.0, 80.0 + (i / 3) as f32 * 90.0, 110.0, 60.0);
            let paint = theme.paint(*s);
            scene.paint(rect, theme.control_radius(), &paint);
            let t = r.text().layout(name, &TextStyle { size: 14.0, weight: 700, ..Default::default() }, None);
            scene.text(&t, Point::new(rect.center().x - t.size().w / 2.0, rect.center().y - t.size().h / 2.0), paint.content);
        }

        // Ring gauge.
        let ring = Rect::new(x0 + 300.0, 180.0, 110.0, 110.0);
        scene.paint(ring, 55.0, &theme.paint(Surface::Raised));
        scene.arc(ring.inset(10.0), 9.0, 0.0, std::f32::consts::TAU, p.well);
        scene.arc(ring.inset(10.0), 9.0, 0.0, std::f32::consts::TAU * 0.62, p.accent);
        let pct = r.text().layout("62%", &TextStyle { size: 18.0, weight: 500, family: FontFamily::Mono, ..Default::default() }, None);
        scene.text(&pct, Point::new(ring.center().x - pct.size().w / 2.0, ring.center().y - pct.size().h / 2.0), p.text);

        // Sparkline.
        let chart = Rect::new(x0 + 30.0, 320.0, 390.0, 90.0);
        scene.paint(chart, theme.control_radius(), &theme.paint(Surface::Well));
        let pts: Vec<Point> = (0..40)
            .map(|i| Point::new(chart.x + 10.0 + i as f32 * 370.0 / 39.0, chart.y + 50.0 - 25.0 * ((i as f32) / 4.0).sin() - (i % 5) as f32 * 3.0))
            .collect();
        scene.area(&pts, chart.bottom() - 8.0, p.accent.with_alpha(0.28), p.accent.with_alpha(0.0));
        scene.polyline(&pts, 2.0, p.accent);

        // Icons.
        let icons: String = [neo_theme::icons::PLAY, neo_theme::icons::SKIP_FORWARD, neo_theme::icons::HEART, neo_theme::icons::SETTINGS, neo_theme::icons::WIFI]
            .iter()
            .map(|i| i.0)
            .collect::<Vec<_>>()
            .iter()
            .flat_map(|c| [*c, ' '])
            .collect();
        let ic = r.text().layout(&icons, &TextStyle { size: 22.0, family: FontFamily::Icons, ..Default::default() }, None);
        scene.text(&ic, Point::new(x0 + 30.0, 440.0), p.accent);
    }

    // Glass panel straddling both halves.
    let glass = Rect::new(330.0, 430.0, 240.0, 70.0);
    scene.backdrop(glass, 16.0, 24.0, Color::WHITE.with_alpha(0.35));
    scene.fill(glass, 16.0, Color::TRANSPARENT, Some((1.0, Color::WHITE.with_alpha(0.5))));
    let gt = r.text().layout("Frosted glass", &TextStyle { size: 16.0, weight: 700, ..Default::default() }, None);
    scene.text(&gt, Point::new(glass.center().x - gt.size().w / 2.0, glass.center().y - gt.size().h / 2.0), Color::hex(0x2B3240));

    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/primitives.png");
    r.save_png(&scene, (w * scale) as u32, (h * scale) as u32, scale, &out).unwrap();
    println!("wrote {}", out.display());
}
