use image::{DynamicImage, Rgba, RgbaImage};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    process::Command,
};

fn avatar_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn avatar_cache_path(url: &str) -> Option<PathBuf> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return None;
    }
    Some(
        dirs::cache_dir()?
            .join("oyo")
            .join("avatars")
            .join(format!("{:016x}.img", avatar_hash(url))),
    )
}

pub(crate) fn cache_avatar_url(url: &str) -> Option<PathBuf> {
    let path = avatar_cache_path(url)?;
    if path.is_file() {
        return Some(path);
    }
    fs::create_dir_all(path.parent()?).ok()?;
    let tmp = path.with_extension("tmp");
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", "5", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .ok()?;
    if !status.success() {
        let _ = fs::remove_file(&tmp);
        return None;
    }
    fs::rename(&tmp, &path).ok()?;
    Some(path)
}

pub(crate) fn avatar_image(url: Option<&str>, seed: &str) -> DynamicImage {
    if let Some(path) = url
        .and_then(avatar_cache_path)
        .filter(|path| path.is_file())
    {
        if let Ok(reader) = image::ImageReader::open(path) {
            if let Ok(reader) = reader.with_guessed_format() {
                if let Ok(image) = reader.decode() {
                    return image;
                }
            }
        }
    }
    default_avatar(seed)
}

fn default_avatar(seed: &str) -> DynamicImage {
    const CELLS: u32 = 5;
    const CELL: u32 = 12;
    const PAD: u32 = 2;
    let size = CELLS * CELL + PAD * 2;
    let hash = avatar_hash(seed);
    let hue = (hash % 360) as f32;
    let fg = hsl_to_rgb(hue, 0.58, 0.55);
    let bg = hsl_to_rgb(hue, 0.25, 0.22);
    let mut image = RgbaImage::from_pixel(size, size, Rgba([bg.0, bg.1, bg.2, 255]));
    for y in 0..CELLS {
        for x in 0..3 {
            let bit = (hash >> (y * 3 + x)) & 1 == 1;
            if !bit {
                continue;
            }
            paint_cell(&mut image, x, y, fg);
            paint_cell(&mut image, CELLS - 1 - x, y, fg);
        }
    }
    DynamicImage::ImageRgba8(image)
}

fn paint_cell(image: &mut RgbaImage, x: u32, y: u32, color: (u8, u8, u8)) {
    const CELL: u32 = 12;
    const PAD: u32 = 2;
    let start_x = PAD + x * CELL;
    let start_y = PAD + y * CELL;
    for py in start_y..start_y + CELL {
        for px in start_x..start_x + CELL {
            image.put_pixel(px, py, Rgba([color.0, color.1, color.2, 255]));
        }
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    )
}
