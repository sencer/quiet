pub mod desktop;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use tracing::{debug, warn};

pub const MAX_ICON_CACHE_ENTRIES: usize = 256;

pub struct IconCache {
    cache: RwLock<HashMap<String, Option<Pixmap>>>,
    theme_name: Option<String>,
    target_size: u32,
}

impl IconCache {
    pub fn new(theme_name: Option<String>, target_size: u32) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            theme_name,
            target_size,
        }
    }

    pub fn target_size(&self) -> u32 {
        self.target_size
    }

    /// Stores a pre-computed or decoded Pixmap directly into the icon cache.
    pub fn insert_pixmap(&self, key: String, pixmap: Pixmap) {
        if let Ok(mut c) = self.cache.write() {
            if c.len() >= MAX_ICON_CACHE_ENTRIES {
                c.retain(|k, _| k.starts_with("raw-image:"));
                if c.len() >= MAX_ICON_CACHE_ENTRIES {
                    c.clear();
                }
            }
            c.insert(key, Some(pixmap));
        }
    }

    /// Removes an icon entry from the cache.
    pub fn remove(&self, key: &str) {
        if let Ok(mut c) = self.cache.write() {
            c.remove(key);
        }
    }

    /// Pre-warms the cache with an icon to ensure instant zero-latency UI rendering.
    pub fn preload(&self, icon_name_or_path: &str) {
        if icon_name_or_path.is_empty() {
            return;
        }
        let key = icon_name_or_path.trim();
        {
            if let Ok(c) = self.cache.read() {
                if c.contains_key(key) {
                    return;
                }
            }
        }
        let loaded = self.load_icon(key);
        if let Ok(mut c) = self.cache.write() {
            if c.len() >= MAX_ICON_CACHE_ENTRIES {
                c.retain(|k, _| k.starts_with("raw-image:"));
                if c.len() >= MAX_ICON_CACHE_ENTRIES {
                    c.clear();
                }
            }
            c.insert(key.to_string(), loaded);
        }
    }

    /// Retrieves an icon from memory cache, or loads it on-demand.
    pub fn get(&self, icon_name_or_path: &str) -> Option<Pixmap> {
        if icon_name_or_path.is_empty() {
            return None;
        }

        let key = icon_name_or_path.trim();

        if let Ok(c) = self.cache.read() {
            if let Some(cached) = c.get(key) {
                return cached.clone();
            }
        }

        let loaded = self.load_icon(key);

        if let Ok(mut c) = self.cache.write() {
            if c.len() >= MAX_ICON_CACHE_ENTRIES {
                c.retain(|k, _| k.starts_with("raw-image:"));
                if c.len() >= MAX_ICON_CACHE_ENTRIES {
                    c.clear();
                }
            }
            c.insert(key.to_string(), loaded.clone());
        }

        loaded
    }

    fn load_icon(&self, name_or_path: &str) -> Option<Pixmap> {
        let path = find_icon_path(name_or_path, self.theme_name.as_deref())?;
        rasterize_icon(&path, self.target_size)
    }
}

/// Decodes raw image-data from Freedesktop notification hints (iiibiiay) into a square Pixmap.
#[allow(clippy::too_many_arguments)]
pub fn pixmap_from_image_data(
    width: i32,
    height: i32,
    rowstride: i32,
    has_alpha: bool,
    bits_per_sample: i32,
    channels: i32,
    data: &[u8],
    target_size: u32,
) -> Option<Pixmap> {
    if width <= 0 || height <= 0 || width > 4096 || height > 4096 {
        return None;
    }
    if bits_per_sample != 8 || (channels != 3 && channels != 4) {
        return None;
    }
    let min_rowstride = width.checked_mul(channels)?;
    if rowstride < min_rowstride {
        return None;
    }

    let required_len = ((height - 1) as usize)
        .checked_mul(rowstride as usize)?
        .checked_add((width as usize) * (channels as usize))?;
    if data.len() < required_len {
        return None;
    }

    let mut src_pixmap = Pixmap::new(width as u32, height as u32)?;
    let pixels = src_pixmap.pixels_mut();

    for y in 0..height as usize {
        let row_start = y * (rowstride as usize);
        let dst_row_start = y * (width as usize);
        for x in 0..width as usize {
            let offset = row_start + x * (channels as usize);
            let r = data[offset];
            let g = data[offset + 1];
            let b = data[offset + 2];
            let a = if has_alpha && channels >= 4 {
                data[offset + 3]
            } else {
                255
            };
            let c = if a == 255 {
                tiny_skia::PremultipliedColorU8::from_rgba(r, g, b, 255)
                    .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT)
            } else if a == 0 {
                tiny_skia::PremultipliedColorU8::TRANSPARENT
            } else {
                let alpha = a as u32;
                let pr = ((r as u32 * alpha + 127) / 255) as u8;
                let pg = ((g as u32 * alpha + 127) / 255) as u8;
                let pb = ((b as u32 * alpha + 127) / 255) as u8;
                tiny_skia::PremultipliedColorU8::from_rgba(pr, pg, pb, a)
                    .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT)
            };
            pixels[dst_row_start + x] = c;
        }
    }

    if width as u32 == target_size && height as u32 == target_size {
        return Some(src_pixmap);
    }

    let mut scaled = Pixmap::new(target_size, target_size)?;
    let src_w = width as f32;
    let src_h = height as f32;
    let scale = ((target_size as f32) / src_w).min((target_size as f32) / src_h);
    let offset_x = ((target_size as f32) - (src_w * scale)) / 2.0;
    let offset_y = ((target_size as f32) - (src_h * scale)) / 2.0;
    let transform = Transform::from_scale(scale, scale).post_translate(offset_x, offset_y);

    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..Default::default()
    };
    scaled.draw_pixmap(0, 0, src_pixmap.as_ref(), &paint, transform, None);
    Some(scaled)
}

/// Resolves an icon name or file path to an absolute path on disk.
pub fn find_icon_path(name_or_path: &str, theme_name: Option<&str>) -> Option<PathBuf> {
    let clean = name_or_path.trim();
    if clean.is_empty() {
        return None;
    }

    // 1. Check direct file or file:// URI
    let direct_path = if let Some(stripped) = clean.strip_prefix("file://") {
        PathBuf::from(stripped)
    } else {
        PathBuf::from(clean)
    };

    if direct_path.is_absolute() && direct_path.is_file() {
        return Some(direct_path);
    }

    // 2. Build list of themes to search in priority order
    let mut themes = Vec::new();
    if let Some(t) = theme_name {
        themes.push(t);
    }
    // Standard themes and fallbacks per XDG Icon Theme specification
    for fallback in &["Papirus-Dark", "Papirus", "Yaru", "Adwaita", "hicolor"] {
        if !themes.contains(fallback) {
            themes.push(fallback);
        }
    }

    // Standard icon base directories
    let mut base_dirs = Vec::new();
    if let Some(data_dir) = dirs::data_dir() {
        base_dirs.push(data_dir.join("icons"));
    }
    if let Some(home) = dirs::home_dir() {
        base_dirs.push(home.join(".icons"));
    }
    base_dirs.push(PathBuf::from("/usr/local/share/icons"));
    base_dirs.push(PathBuf::from("/usr/share/icons"));

    // Subdirectories in preference order for ~32px card rendering
    let subdirs = [
        "32x32/apps",
        "48x48/apps",
        "24x24/apps",
        "64x64/apps",
        "scalable/apps",
        "32x32/status",
        "48x48/status",
        "scalable/status",
        "32x32/panel",
        "24x24/panel",
        "symbolic/apps",
    ];

    let icon_stem = clean
        .strip_suffix(".png")
        .or_else(|| clean.strip_suffix(".svg"))
        .unwrap_or(clean);

    for theme in &themes {
        for base in &base_dirs {
            let theme_dir = base.join(theme);
            if !theme_dir.exists() {
                continue;
            }

            for subdir in &subdirs {
                let dir = theme_dir.join(subdir);
                // Prefer SVG for crisp scaling, fallback to PNG
                let svg_path = dir.join(format!("{}.svg", icon_stem));
                if svg_path.is_file() {
                    return Some(svg_path);
                }
                let png_path = dir.join(format!("{}.png", icon_stem));
                if png_path.is_file() {
                    return Some(png_path);
                }
            }
        }
    }

    // 3. Check fallback /usr/share/pixmaps
    let pixmaps = Path::new("/usr/share/pixmaps");
    if pixmaps.exists() {
        let svg = pixmaps.join(format!("{}.svg", icon_stem));
        if svg.is_file() {
            return Some(svg);
        }
        let png = pixmaps.join(format!("{}.png", icon_stem));
        if png.is_file() {
            return Some(png);
        }
        let direct_in_pixmaps = pixmaps.join(clean);
        if direct_in_pixmaps.is_file() {
            return Some(direct_in_pixmaps);
        }
    }

    debug!("Could not resolve icon path for {:?}", name_or_path);
    None
}

/// Decodes and rasterizes an icon file (PNG or SVG) into a square `target_size` Pixmap.
pub fn rasterize_icon(path: &Path, target_size: u32) -> Option<Pixmap> {
    let extension = path.extension()?.to_str()?.to_lowercase();
    let bytes = fs::read(path).ok()?;

    if extension == "svg" {
        rasterize_svg(&bytes, target_size)
    } else if extension == "png" {
        decode_and_scale_png(&bytes, target_size)
    } else {
        warn!(
            "Unsupported icon file format {:?} for {:?}",
            extension, path
        );
        None
    }
}

fn rasterize_svg(svg_data: &[u8], target_size: u32) -> Option<Pixmap> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg_data, &opt).ok()?;

    let svg_w = tree.size().width();
    let svg_h = tree.size().height();

    if svg_w <= 0.0 || svg_h <= 0.0 {
        return None;
    }

    let mut pixmap = Pixmap::new(target_size, target_size)?;

    // Calculate aspect-preserving scale factor
    let scale = ((target_size as f32) / svg_w).min((target_size as f32) / svg_h);
    let offset_x = ((target_size as f32) - (svg_w * scale)) / 2.0;
    let offset_y = ((target_size as f32) - (svg_h * scale)) / 2.0;

    // Scale first, then translate into centered bounding box
    let transform = Transform::from_scale(scale, scale).post_translate(offset_x, offset_y);

    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(pixmap)
}

fn decode_and_scale_png(png_data: &[u8], target_size: u32) -> Option<Pixmap> {
    let source = Pixmap::decode_png(png_data).ok()?;

    if source.width() == target_size && source.height() == target_size {
        return Some(source);
    }

    let mut scaled = Pixmap::new(target_size, target_size)?;
    let src_w = source.width() as f32;
    let src_h = source.height() as f32;

    let scale = ((target_size as f32) / src_w).min((target_size as f32) / src_h);
    let offset_x = ((target_size as f32) - (src_w * scale)) / 2.0;
    let offset_y = ((target_size as f32) - (src_h * scale)) / 2.0;

    // Scale first, then translate into centered bounding box
    let transform = Transform::from_scale(scale, scale).post_translate(offset_x, offset_y);

    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..Default::default()
    };

    scaled.draw_pixmap(0, 0, source.as_ref(), &paint, transform, None);
    Some(scaled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_icon_path_fallback() {
        let path = find_icon_path("debian-logo", None);
        if Path::new("/usr/share/pixmaps/debian-logo.png").exists() {
            assert!(path.is_some());
        }
    }

    #[test]
    fn test_rasterize_simple_svg() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="red"/></svg>"#;
        let pixmap = rasterize_svg(svg.as_bytes(), 32);
        assert!(pixmap.is_some());
        let p = pixmap.unwrap();
        assert_eq!(p.width(), 32);
        assert_eq!(p.height(), 32);
    }

    #[test]
    fn test_image_data_premultiplication() {
        // Pixel with r=200, g=50, b=50, a=100 (where r > a, so straight RGBA)
        let data = vec![200u8, 50u8, 50u8, 100u8];
        let pixmap = pixmap_from_image_data(1, 1, 4, true, 8, 4, &data, 1);
        assert!(pixmap.is_some());
        let p = pixmap.unwrap();
        let pixel = p.pixel(0, 0).unwrap();
        assert_eq!(pixel.alpha(), 100);
        // Premultiplied red should be around (200 * 100 + 127) / 255 = 78
        assert_eq!(pixel.red(), 78);
    }

    #[test]
    fn test_icon_cache_retains_raw_images() {
        let cache = IconCache::new(None, 32);
        let dummy = Pixmap::new(32, 32).unwrap();
        cache.insert_pixmap("raw-image:42".into(), dummy.clone());

        // Fill cache up to limit with normal icons
        for i in 0..MAX_ICON_CACHE_ENTRIES + 10 {
            cache.insert_pixmap(format!("icon-{}", i), dummy.clone());
        }

        // Verify raw-image:42 was preserved despite eviction
        assert!(cache.get("raw-image:42").is_some());
    }
}
