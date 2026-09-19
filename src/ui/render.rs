use crate::config::Config;
use crate::engine::UiItem;
use cosmic_text::{
    Attrs, Buffer, Color as CosmicColor, FontSystem, Metrics, Shaping, SwashCache, Weight,
};
use tiny_skia::{Color as SkiaColor, FillRule, Paint, PathBuilder, PixmapMut, Transform};

use std::sync::Arc;

pub struct Renderer {
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub icon_cache: Arc<crate::icon::IconCache>,
    text_buffer: Buffer,
}

impl Renderer {
    pub fn new(icon_cache: Arc<crate::icon::IconCache>) -> Self {
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_system_fonts();
        let swash_cache = SwashCache::new();
        let text_buffer = Buffer::new(&mut font_system, Metrics::new(14.0, 18.0));
        Self {
            font_system,
            swash_cache,
            icon_cache,
            text_buffer,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pixmap: &mut PixmapMut,
        items: &[UiItem],
        selected_index: usize,
        scroll_offset: usize,
        _context: &[String],
        config: &Config,
        scale: f32,
    ) {
        let width = pixmap.width() as f32;
        let height = pixmap.height() as f32;
        let scale = scale.max(1.0);

        // Fill window canvas with configured background (defaults to 100% transparent in widget mode)
        let bg_color =
            parse_skia_color(&config.ui.bg_color, SkiaColor::TRANSPARENT);
        pixmap.fill(bg_color);

        if width < 40.0 * scale || height < 40.0 * scale {
            return;
        }

        let padding_x = 10.0 * scale;
        let padding_y = 10.0 * scale;
        let card_height = 98.0 * scale;
        let card_spacing = 10.0 * scale;
        let border_radius = 3.0 * scale;

        // Empty state
        if items.is_empty() {
            let card_x = padding_x;
            let card_w = width - padding_x * 2.0;
            let card_rect = rounded_rect(card_x, padding_y, card_w, card_height, border_radius);
            let mut card_paint = Paint::default();
            card_paint.set_color(SkiaColor::from_rgba8(76, 76, 76, 255)); // #4c4c4c
            card_paint.anti_alias = true;
            if let Some(ref rect_path) = card_rect {
                pixmap.fill_path(rect_path, &card_paint, FillRule::Winding, Transform::identity(), None);
            }
            let font_size = config.ui.font_size * scale;
            self.draw_styled_text(
                pixmap,
                "No notifications",
                card_x + 16.0 * scale,
                padding_y + (card_height - font_size) / 2.0,
                CosmicColor::rgb(221, 204, 187), // #ddccbb
                font_size,
                false,
                false,
                card_w - 32.0 * scale,
                config,
                scale,
            );
            return;
        }

        let available_h = height - padding_y * 2.0;
        let max_visible_cards =
            (((available_h + card_spacing) / (card_height + card_spacing)).floor() as usize).max(1);

        let has_overflow = items.len() > max_visible_cards;
        let card_x = padding_x;
        let card_w = if has_overflow {
            (width - padding_x * 2.0 - 6.0 * scale).max(10.0)
        } else {
            (width - padding_x * 2.0).max(10.0)
        };

        // Scrollbar if overflowing
        if has_overflow {
            let track_w = 4.0 * scale;
            let track_x = width - 7.0 * scale;
            let track_y = padding_y;
            let track_h = height - padding_y * 2.0;

            let mut track_paint = Paint::default();
            track_paint.set_color(SkiaColor::from_rgba8(30, 30, 30, 150));
            track_paint.anti_alias = true;
            if let Some(track_path) = rounded_rect(track_x, track_y, track_w, track_h, 2.0 * scale) {
                pixmap.fill_path(&track_path, &track_paint, FillRule::Winding, Transform::identity(), None);
            }

            let thumb_ratio = (max_visible_cards as f32) / (items.len() as f32);
            let thumb_h = (track_h * thumb_ratio).max(16.0 * scale);
            let max_scroll = items.len().saturating_sub(max_visible_cards).max(1) as f32;
            let scroll_ratio = (scroll_offset as f32 / max_scroll).clamp(0.0, 1.0);
            let thumb_y = track_y + (track_h - thumb_h) * scroll_ratio;

            let mut thumb_paint = Paint::default();
            thumb_paint.set_color(SkiaColor::from_rgba8(135, 206, 235, 200)); // #87ceeb
            thumb_paint.anti_alias = true;
            if let Some(thumb_path) = rounded_rect(track_x, thumb_y, track_w, thumb_h, 2.0 * scale) {
                pixmap.fill_path(&thumb_path, &thumb_paint, FillRule::Winding, Transform::identity(), None);
            }
        }

        let mut curr_y = padding_y;

        for (idx, item) in items.iter().enumerate().skip(scroll_offset) {
            if curr_y + card_height > height + 1.0 {
                break;
            }

            let is_selected = idx == selected_index;
            let card_rect = rounded_rect(card_x, curr_y, card_w, card_height, border_radius);

            // Card background colors matching rofi common.rasi exactly
            let card_bg = if is_selected {
                if item.urgency == 2 {
                    // Selected urgent: #cd5c5c
                    SkiaColor::from_rgba8(205, 92, 92, 255)
                } else if item.is_group {
                    // Selected group: #87ceeb
                    SkiaColor::from_rgba8(135, 206, 235, 255)
                } else {
                    // Selected normal: #6495ed
                    parse_skia_color(
                        &config.ui.selected_bg_color,
                        SkiaColor::from_rgba8(100, 149, 237, 255),
                    )
                }
            } else if item.urgency == 2 {
                // Urgent: #cc5533
                parse_skia_color(
                    &config.ui.urgent_bg_color,
                    SkiaColor::from_rgba8(204, 85, 51, 255),
                )
            } else if item.is_group {
                // Group: #4c4c6c
                parse_skia_color(
                    &config.ui.cluster_bg_color,
                    SkiaColor::from_rgba8(76, 76, 108, 255),
                )
            } else {
                // Normal: #4c4c4c
                SkiaColor::from_rgba8(76, 76, 76, 255)
            };

            let mut card_paint = Paint::default();
            card_paint.set_color(card_bg);
            card_paint.anti_alias = true;
            if let Some(ref rect_path) = card_rect {
                pixmap.fill_path(
                    rect_path,
                    &card_paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }

            // Text colors matching rofi common.rasi exactly
            let (title_color, meta_color, body_color) = if is_selected {
                (
                    CosmicColor::rgb(28, 28, 28), // #1c1c1c (Dark text on selection)
                    CosmicColor::rgb(55, 55, 55),
                    CosmicColor::rgb(40, 40, 40),
                )
            } else if item.urgency == 2 {
                (
                    CosmicColor::rgb(255, 255, 255),
                    CosmicColor::rgb(255, 230, 230),
                    CosmicColor::rgb(255, 240, 240),
                )
            } else {
                (
                    parse_cosmic_color(&config.ui.text_color, CosmicColor::rgb(221, 204, 187)), // #ddccbb (Light White)
                    CosmicColor::rgb(170, 160, 145), // Muted time & app name
                    CosmicColor::rgb(205, 195, 180), // Body text
                )
            };

            let has_icon = config.ui.show_icons && !item.icon.is_empty();
            let mut icon_rendered = false;
            let target_icon_size = (config.ui.icon_size as f32 * scale).min(card_height - 24.0 * scale);

            if has_icon {
                if let Some(icon_pixmap) = self.icon_cache.get(&item.icon) {
                    let icon_x = card_x + 15.0 * scale;
                    let icon_y = curr_y + (card_height - target_icon_size) / 2.0;

                    let scale_x = target_icon_size / (icon_pixmap.width() as f32);
                    let scale_y = target_icon_size / (icon_pixmap.height() as f32);
                    let transform =
                        Transform::from_scale(scale_x, scale_y).post_translate(icon_x, icon_y);

                    let paint = tiny_skia::PixmapPaint {
                        quality: tiny_skia::FilterQuality::Bilinear,
                        ..Default::default()
                    };
                    pixmap.draw_pixmap(0, 0, icon_pixmap.as_ref(), &paint, transform, None);
                    icon_rendered = true;
                }
            }

            let text_left = if icon_rendered {
                card_x + 15.0 * scale + target_icon_size + 15.0 * scale
            } else {
                card_x + 15.0 * scale
            };
            let max_text_w = (card_x + card_w - text_left - 15.0 * scale).max(10.0);

            let font_size = config.ui.font_size * scale;
            let line1_y = curr_y + 18.0 * scale;

            // Line 1: <b>Summary</b> <small>Time</small> <small>AppName</small>
            let summary_text = if !item.summary.is_empty() {
                &item.summary
            } else {
                &item.title
            };

            let summary_w = self.draw_styled_text(
                pixmap,
                summary_text,
                text_left,
                line1_y,
                title_color,
                font_size,
                true,  // Bold
                false, // Italic
                max_text_w,
                config,
                scale,
            );

            let meta_x = text_left + summary_w + 10.0 * scale;
            let meta_max_w = (card_x + card_w - meta_x - 15.0 * scale).max(0.0);
            if meta_max_w > 12.0 * scale {
                let meta_str = if !item.time_str.is_empty() && !item.app_name.is_empty() {
                    format!("{} {}", item.time_str, item.app_name)
                } else if !item.time_str.is_empty() {
                    item.time_str.clone()
                } else {
                    item.app_name.clone()
                };
                self.draw_styled_text(
                    pixmap,
                    &meta_str,
                    meta_x,
                    line1_y + 2.0 * scale,
                    meta_color,
                    (font_size - 3.0).max(8.0),
                    false, // Regular
                    false, // Italic
                    meta_max_w,
                    config,
                    scale,
                );
            }

            // Line 2: <i>Body</i>
            let line2_y = curr_y + 54.0 * scale;
            let body_source = if !item.body.is_empty() {
                &item.body
            } else {
                &item.subtitle
            };
            let body_single_line = body_source.replace('\n', " ").trim().to_string();
            let preview = if body_single_line.is_empty() {
                if item.is_group {
                    format!("{} notification(s)", item.count)
                } else {
                    String::new()
                }
            } else {
                body_single_line
            };

            if !preview.is_empty() {
                self.draw_styled_text(
                    pixmap,
                    &preview,
                    text_left,
                    line2_y,
                    body_color,
                    (font_size - 1.0).max(8.0),
                    false, // Regular
                    true,  // Italic
                    max_text_w,
                    config,
                    scale,
                );
            }

            curr_y += card_height + card_spacing;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_styled_text(
        &mut self,
        pixmap: &mut PixmapMut,
        text: &str,
        x: f32,
        y: f32,
        color: CosmicColor,
        font_size: f32,
        bold: bool,
        italic: bool,
        max_width: f32,
        config: &Config,
        scale: f32,
    ) -> f32 {
        let max_width = max_width.max(10.0);
        let line_height = font_size + 4.0 * scale;
        self.text_buffer.set_metrics_and_size(
            &mut self.font_system,
            Metrics::new(font_size, line_height),
            Some(max_width),
            Some(line_height),
        );

        let mut attrs = Attrs::new().family(cosmic_text::Family::Name(&config.ui.font_family));
        if bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if italic {
            attrs = attrs.style(cosmic_text::Style::Italic);
        }

        self.text_buffer.set_text(&mut self.font_system, text, attrs, Shaping::Advanced);
        self.text_buffer.shape_until_scroll(&mut self.font_system, false);

        let pw = pixmap.width() as i32;
        let ph = pixmap.height() as i32;
        let base_x = x.round() as i32;
        let base_y = y.round() as i32;
        let max_w_i32 = max_width.ceil() as i32;
        let line_h_i32 = line_height.ceil() as i32;
        let pixels = pixmap.pixels_mut();

        self.text_buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            color,
            |gx, gy, gw, gh, glyph_color| {
                let glyph_a = glyph_color.a() as u32;
                if glyph_a == 0 || gy >= line_h_i32 {
                    return;
                }
                let inv_glyph_a = 255 - glyph_a;
                let sr = (glyph_color.r() as u32 * glyph_a) / 255;
                let sg = (glyph_color.g() as u32 * glyph_a) / 255;
                let sb = (glyph_color.b() as u32 * glyph_a) / 255;

                for dy in 0..gh as i32 {
                    let py = base_y + gy + dy;
                    if py < 0 || py >= ph || gy + dy >= line_h_i32 {
                        continue;
                    }
                    for dx in 0..gw as i32 {
                        let rel_x = gx + dx;
                        if rel_x < 0 || rel_x >= max_w_i32 {
                            continue;
                        }
                        let px = base_x + rel_x;
                        if px < 0 || px >= pw {
                            continue;
                        }
                        let idx = (py * pw + px) as usize;
                        let c = pixels[idx];
                        let orig_a = c.alpha() as u32;

                        let out_r = (sr + (c.red() as u32 * inv_glyph_a) / 255).min(255) as u8;
                        let out_g = (sg + (c.green() as u32 * inv_glyph_a) / 255).min(255) as u8;
                        let out_b = (sb + (c.blue() as u32 * inv_glyph_a) / 255).min(255) as u8;
                        let out_a = (glyph_a + (orig_a * inv_glyph_a) / 255).min(255) as u8;

                        pixels[idx] = tiny_skia::PremultipliedColorU8::from_rgba(out_r, out_g, out_b, out_a)
                            .unwrap_or(c);
                    }
                }
            },
        );

        self.text_buffer.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0)
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

pub fn parse_skia_color(hex: &str, fallback: SkiaColor) -> SkiaColor {
    let s = hex.trim().trim_start_matches('#');
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
            SkiaColor::from_rgba8(r, g, b, 255)
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
            let a = u8::from_str_radix(&s[6..8], 16).unwrap_or(255);
            SkiaColor::from_rgba8(r, g, b, a)
        }
        _ => fallback,
    }
}

pub fn parse_cosmic_color(hex: &str, fallback: CosmicColor) -> CosmicColor {
    let s = hex.trim().trim_start_matches('#');
    match s.len() {
        6 | 8 => {
            let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(240);
            let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(240);
            let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(240);
            CosmicColor::rgb(r, g, b)
        }
        _ => fallback,
    }
}
