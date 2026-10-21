//! Card pictures for the panels (generate shapes, looks, world art, map styles):
//! `assets/onboarding/<key>.jpg`, decoded off the frame thread at card size and cached; a few
//! shapes without a picture (ring atoll, empty ocean, flat plains) are painted here.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use bevy_egui::egui::{self, Color32, ColorImage, TextureHandle, TextureId};

enum Slot {
    Loading,
    Ready(TextureHandle),
    Missing,
}

struct Loader {
    slots: HashMap<String, Slot>,
    tx: Sender<(String, Option<ColorImage>)>,
    rx: Receiver<(String, Option<ColorImage>)>,
}

static LOADER: Mutex<Option<Loader>> = Mutex::new(None);

/// The card picture `key` (an onboarding jpg, or a painted `gen:` preview); None while it loads.
pub fn get(ctx: &egui::Context, key: &str) -> Option<TextureId> {
    let mut guard = LOADER.lock().ok()?;
    let l = guard.get_or_insert_with(|| {
        let (tx, rx) = channel();
        Loader {
            slots: HashMap::new(),
            tx,
            rx,
        }
    });
    while let Ok((k, img)) = l.rx.try_recv() {
        let slot = match img {
            Some(img) => Slot::Ready(ctx.load_texture(
                format!("card_{k}"),
                img,
                egui::TextureOptions::LINEAR,
            )),
            None => Slot::Missing,
        };
        l.slots.insert(k, slot);
        ctx.request_repaint();
    }
    match l.slots.get(key) {
        Some(Slot::Ready(t)) => return Some(t.id()),
        Some(_) => return None,
        None => {}
    }
    if let Some(img) = painted(key) {
        let t = ctx.load_texture(format!("card_{key}"), img, egui::TextureOptions::LINEAR);
        let id = t.id();
        l.slots.insert(key.to_string(), Slot::Ready(t));
        return Some(id);
    }
    l.slots.insert(key.to_string(), Slot::Loading);
    let tx = l.tx.clone();
    let path = std::path::PathBuf::from(crate::assets_path())
        .join("onboarding")
        .join(format!("{key}.jpg"));
    let key = key.to_string();
    std::thread::spawn(move || {
        let img = image::open(&path).ok().map(|im| {
            let im = if im.width() > 360 {
                im.resize(360, 360, image::imageops::FilterType::Triangle)
            } else {
                im
            };
            let rgba = im.to_rgba8();
            ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            )
        });
        let _ = tx.send((key, img));
    });
    None
}

/// Previews painted in code for shapes without a picture.
fn painted(key: &str) -> Option<ColorImage> {
    let n = 128usize;
    let sea = |d: f32| {
        let k = (1.0 - d * 0.35).clamp(0.0, 1.0);
        [
            (12.0 + 10.0 * k) as u8,
            (48.0 + 34.0 * k) as u8,
            (86.0 + 44.0 * k) as u8,
        ]
    };
    let px: Vec<Color32> = match key {
        "gen:ring_atoll" => (0..n * n)
            .map(|i| {
                let (x, y) = (
                    (i % n) as f32 / n as f32 - 0.5,
                    (i / n) as f32 / n as f32 - 0.5,
                );
                let r = (x * x + y * y).sqrt();
                let ring = (r - 0.32).abs();
                if ring < 0.022 {
                    Color32::from_rgb(196, 184, 132)
                } else if ring < 0.05 {
                    Color32::from_rgb(52, 150, 160)
                } else if r < 0.3 {
                    Color32::from_rgb(38, 120, 150)
                } else {
                    let c = sea(r);
                    Color32::from_rgb(c[0], c[1], c[2])
                }
            })
            .collect(),
        "gen:empty_ocean" => (0..n * n)
            .map(|i| {
                let (x, y) = (
                    (i % n) as f32 / n as f32 - 0.5,
                    (i / n) as f32 / n as f32 - 0.5,
                );
                let c = sea((x * x + y * y).sqrt() * 1.4);
                Color32::from_rgb(c[0], c[1], c[2])
            })
            .collect(),
        "gen:flat_plains" => (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f32 / n as f32, (i / n) as f32 / n as f32);
                let w = ((x * 9.0).sin() * (y * 7.0).cos() * 0.5 + 0.5) * 18.0;
                Color32::from_rgb((86.0 + w) as u8, (128.0 + w) as u8, (62.0 + w * 0.5) as u8)
            })
            .collect(),
        _ => return None,
    };
    Some(ColorImage::new([n, n], px))
}
