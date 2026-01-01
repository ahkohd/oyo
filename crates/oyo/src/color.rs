//! Color types and HSL-based gradient interpolation for animations.

use crate::app::AnimationPhase;
use ratatui::style::Color;
use std::collections::HashMap;

/// RGB color (0-255 per channel)
#[derive(Debug, Clone, Copy)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// HSL color (h: 0-360, s: 0-1, l: 0-1)
#[derive(Debug, Clone, Copy)]
pub struct Hsl {
    pub h: f32,
    pub s: f32,
    pub l: f32,
}

/// 3-stop gradient: neutral → base → bright
#[derive(Debug, Clone, Copy)]
pub struct AnimationGradient {
    pub neutral: Hsl,
    pub base: Hsl,
    pub bright: Hsl,
}

/// Parse hex color string (e.g., "#2ecc71" or "2ecc71")
pub fn parse_hex(s: &str) -> Result<Rgb, String> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return Err(format!(
            "invalid hex color: expected 6 characters, got {}",
            s.len()
        ));
    }

    let r = u8::from_str_radix(&s[0..2], 16)
        .map_err(|_| format!("invalid hex color: bad red component in '{}'", s))?;
    let g = u8::from_str_radix(&s[2..4], 16)
        .map_err(|_| format!("invalid hex color: bad green component in '{}'", s))?;
    let b = u8::from_str_radix(&s[4..6], 16)
        .map_err(|_| format!("invalid hex color: bad blue component in '{}'", s))?;

    Ok(Rgb { r, g, b })
}

/// Convert RGB to HSL
pub fn rgb_to_hsl(rgb: Rgb) -> Hsl {
    let r = rgb.r as f32 / 255.0;
    let g = rgb.g as f32 / 255.0;
    let b = rgb.b as f32 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;

    if (max - min).abs() < f32::EPSILON {
        // Achromatic (gray)
        return Hsl { h: 0.0, s: 0.0, l };
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };

    let h = if (max - r).abs() < f32::EPSILON {
        let mut h = (g - b) / d;
        if g < b {
            h += 6.0;
        }
        h
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };

    Hsl {
        h: (h * 60.0).rem_euclid(360.0),
        s: s.clamp(0.0, 1.0),
        l: l.clamp(0.0, 1.0),
    }
}

/// Convert HSL to RGB
pub fn hsl_to_rgb(hsl: Hsl) -> Rgb {
    let h = hsl.h.rem_euclid(360.0);
    let s = hsl.s.clamp(0.0, 1.0);
    let l = hsl.l.clamp(0.0, 1.0);

    if s.abs() < f32::EPSILON {
        // Achromatic (gray)
        let v = (l * 255.0).round() as u8;
        return Rgb { r: v, g: v, b: v };
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;

    fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
        t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 1.0 / 2.0 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    }

    let h_norm = h / 360.0;
    Rgb {
        r: (hue_to_rgb(p, q, h_norm + 1.0 / 3.0) * 255.0).round() as u8,
        g: (hue_to_rgb(p, q, h_norm) * 255.0).round() as u8,
        b: (hue_to_rgb(p, q, h_norm - 1.0 / 3.0) * 255.0).round() as u8,
    }
}

/// Interpolate between two hue values, taking the shortest path around the circle
fn lerp_hue(a: f32, b: f32, t: f32) -> f32 {
    let diff = b - a;
    let delta = if diff > 180.0 {
        diff - 360.0
    } else if diff < -180.0 {
        diff + 360.0
    } else {
        diff
    };
    (a + delta * t).rem_euclid(360.0)
}

/// Linear interpolation
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Interpolate between two HSL colors
pub fn lerp_hsl(a: Hsl, b: Hsl, t: f32) -> Hsl {
    let t = t.clamp(0.0, 1.0);
    Hsl {
        h: lerp_hue(a.h, b.h, t),
        s: lerp(a.s, b.s, t).clamp(0.0, 1.0),
        l: lerp(a.l, b.l, t).clamp(0.0, 1.0),
    }
}

/// Derive a 3-stop gradient from a base color
/// neutral: desaturated, lighter (toward gray)
/// base: as-is
/// bright: lighter, slightly desaturated to avoid neon
pub fn derive_gradient(base: Hsl) -> AnimationGradient {
    let neutral = Hsl {
        h: base.h,
        s: base.s * 0.15,
        l: (base.l + 0.20).min(0.85),
    };

    let bright = Hsl {
        h: base.h,
        s: base.s * 0.75,
        l: (base.l + 0.25).min(0.95),
    };

    AnimationGradient {
        neutral,
        base,
        bright,
    }
}

/// Get color from gradient at position t (0.0 = neutral, 0.5 = base, 1.0 = bright)
pub fn gradient_color(gradient: &AnimationGradient, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);

    let hsl = if t < 0.5 {
        // neutral → base
        lerp_hsl(gradient.neutral, gradient.base, t * 2.0)
    } else {
        // base → bright
        lerp_hsl(gradient.base, gradient.bright, (t - 0.5) * 2.0)
    };

    hsl_to_rgb(hsl)
}

/// Build a single-hue ramp from dark to light for a given base color.
pub fn ramp_color(base: Color, t: f32) -> Color {
    let Some(rgb) = color_to_rgb(base) else {
        return base;
    };
    let mut hsl = rgb_to_hsl(rgb);
    let start_l = (hsl.l * 0.35).clamp(0.05, 0.6);
    let end_l = (hsl.l + 0.35).min(0.9);
    let t = t.clamp(0.0, 1.0);
    hsl.l = start_l + (end_l - start_l) * t;
    let rgb = hsl_to_rgb(hsl);
    Color::Rgb(rgb.r, rgb.g, rgb.b)
}

/// Relative luminance (sRGB) for contrast calculations.
pub fn relative_luminance(color: Color) -> Option<f32> {
    match color {
        Color::Rgb(r, g, b) => {
            let r = r as f32 / 255.0;
            let g = g as f32 / 255.0;
            let b = b as f32 / 255.0;
            let r = if r <= 0.03928 {
                r / 12.92
            } else {
                ((r + 0.055) / 1.055).powf(2.4)
            };
            let g = if g <= 0.03928 {
                g / 12.92
            } else {
                ((g + 0.055) / 1.055).powf(2.4)
            };
            let b = if b <= 0.03928 {
                b / 12.92
            } else {
                ((b + 0.055) / 1.055).powf(2.4)
            };
            Some(0.2126 * r + 0.7152 * g + 0.0722 * b)
        }
        _ => None,
    }
}

/// Contrast ratio between two colors (WCAG formula).
pub fn contrast_ratio(a: Color, b: Color) -> Option<f32> {
    let la = relative_luminance(a)?;
    let lb = relative_luminance(b)?;
    let (max, min) = if la > lb { (la, lb) } else { (lb, la) };
    Some((max + 0.05) / (min + 0.05))
}

/// Compute a linear animation t value across both phases (0.0 → 1.0)
pub fn animation_t_linear(phase: AnimationPhase, progress: f32) -> f32 {
    let p = progress.clamp(0.0, 1.0);
    match phase {
        AnimationPhase::FadeOut => p * 0.5,
        AnimationPhase::FadeIn => 0.5 + p * 0.5,
        AnimationPhase::Idle => 1.0,
    }
}

/// Ease-out curve: fast start, slow end
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(2)
}

/// Linear interpolate between two RGB colors
pub fn lerp_rgb_color(from: Color, to: Color, t: f32) -> Color {
    match (from, to) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let t = t.clamp(0.0, 1.0);
            Color::Rgb(
                ((r1 as f32) + (r2 as f32 - r1 as f32) * t).round() as u8,
                ((g1 as f32) + (g2 as f32 - g1 as f32) * t).round() as u8,
                ((b1 as f32) + (b2 as f32 - b1 as f32) * t).round() as u8,
            )
        }
        _ => to,
    }
}

/// Default gradient colors
pub mod defaults {
    pub const INSERT_HEX: &str = "#2ecc71";
    pub const DELETE_HEX: &str = "#e74c3c";
    pub const MODIFY_HEX: &str = "#f1c40f";
}

// ============================================================================
// Theme color resolution
// ============================================================================

/// Parse ANSI color name to ratatui Color
pub fn parse_ansi_name(name: &str) -> Option<Color> {
    match name.to_lowercase().replace('-', "_").as_str() {
        "default" | "reset" => Some(Color::Reset),
        "transparent" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "dark_gray" | "dark_grey" | "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "light_red" | "lightred" => Some(Color::LightRed),
        "light_green" | "lightgreen" => Some(Color::LightGreen),
        "light_yellow" | "lightyellow" => Some(Color::LightYellow),
        "light_blue" | "lightblue" => Some(Color::LightBlue),
        "light_magenta" | "lightmagenta" => Some(Color::LightMagenta),
        "light_cyan" | "lightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

/// Resolve a color string: def reference, hex, or ANSI name
pub fn resolve_color(value: &str, defs: &HashMap<String, String>) -> Option<Color> {
    let value = value.trim();

    // Check def reference first
    if let Some(hex) = defs.get(value) {
        return parse_hex(hex)
            .ok()
            .map(|rgb| Color::Rgb(rgb.r, rgb.g, rgb.b));
    }

    // Try hex
    if value.starts_with('#') {
        return parse_hex(value)
            .ok()
            .map(|rgb| Color::Rgb(rgb.r, rgb.g, rgb.b));
    }

    // Try ANSI name
    parse_ansi_name(value)
}

/// Resolve color with fallback
#[allow(dead_code)]
pub fn resolve_color_or(value: &str, defs: &HashMap<String, String>, fallback: Color) -> Color {
    resolve_color(value, defs).unwrap_or(fallback)
}

/// Derive a dimmed version of a color via HSL (reduce saturation and lightness)
pub fn dim_color(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => {
            let rgb = Rgb { r, g, b };
            let mut hsl = rgb_to_hsl(rgb);
            hsl.s *= 0.4; // reduce saturation by 60%
            hsl.l *= 0.6; // reduce lightness by 40%
            let dimmed = hsl_to_rgb(hsl);
            Color::Rgb(dimmed.r, dimmed.g, dimmed.b)
        }
        // For ANSI colors, map to darker variant
        Color::Green | Color::LightGreen => Color::DarkGray,
        Color::Red | Color::LightRed => Color::DarkGray,
        Color::Yellow | Color::LightYellow => Color::DarkGray,
        Color::Cyan | Color::LightCyan => Color::DarkGray,
        Color::Blue | Color::LightBlue => Color::DarkGray,
        Color::Magenta | Color::LightMagenta => Color::DarkGray,
        Color::White | Color::Gray => Color::DarkGray,
        _ => color,
    }
}

/// Derive a dimmed color from the base of an animation gradient
pub fn dim_color_from_gradient(gradient: &AnimationGradient) -> Color {
    let base_rgb = hsl_to_rgb(gradient.base);
    dim_color(Color::Rgb(base_rgb.r, base_rgb.g, base_rgb.b))
}

/// Convert ratatui Color to Rgb (for HSL conversion)
/// Returns None for non-RGB colors (ANSI names)
#[allow(dead_code)]
pub fn color_to_rgb(color: Color) -> Option<Rgb> {
    match color {
        Color::Rgb(r, g, b) => Some(Rgb { r, g, b }),
        _ => None,
    }
}

/// Blend two colors using alpha (0.0 = bg, 1.0 = fg).
pub fn blend_colors(bg: Color, fg: Color, alpha: f32) -> Option<Color> {
    let bg = color_to_rgb(bg)?;
    let fg = color_to_rgb(fg)?;
    let a = alpha.clamp(0.0, 1.0);
    let blend = |b: u8, f: u8| -> u8 { (b as f32 * (1.0 - a) + f as f32 * a).round() as u8 };
    Some(Color::Rgb(
        blend(bg.r, fg.r),
        blend(bg.g, fg.g),
        blend(bg.b, fg.b),
    ))
}

/// Build AnimationGradient from a ratatui Color
/// For ANSI colors, uses sensible hex defaults
pub fn gradient_from_color(color: Color) -> AnimationGradient {
    let rgb = match color {
        Color::Rgb(r, g, b) => Rgb { r, g, b },
        Color::Green | Color::LightGreen => parse_hex(defaults::INSERT_HEX).unwrap(),
        Color::Red | Color::LightRed => parse_hex(defaults::DELETE_HEX).unwrap(),
        Color::Yellow | Color::LightYellow => parse_hex(defaults::MODIFY_HEX).unwrap(),
        // Fallback to a neutral gray for other colors
        _ => Rgb {
            r: 128,
            g: 128,
            b: 128,
        },
    };
    derive_gradient(rgb_to_hsl(rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex() {
        let rgb = parse_hex("#2ecc71").unwrap();
        assert_eq!(rgb.r, 46);
        assert_eq!(rgb.g, 204);
        assert_eq!(rgb.b, 113);

        let rgb = parse_hex("e74c3c").unwrap();
        assert_eq!(rgb.r, 231);
        assert_eq!(rgb.g, 76);
        assert_eq!(rgb.b, 60);
    }

    #[test]
    fn test_rgb_hsl_roundtrip() {
        let original = Rgb {
            r: 46,
            g: 204,
            b: 113,
        };
        let hsl = rgb_to_hsl(original);
        let back = hsl_to_rgb(hsl);

        // Allow for rounding errors
        assert!((original.r as i16 - back.r as i16).abs() <= 1);
        assert!((original.g as i16 - back.g as i16).abs() <= 1);
        assert!((original.b as i16 - back.b as i16).abs() <= 1);
    }

    #[test]
    fn test_lerp_hue_wrap() {
        // 350° → 10° should go through 0°, not 180°
        let result = super::lerp_hue(350.0, 10.0, 0.5);
        assert!((result - 0.0).abs() < 1.0 || (result - 360.0).abs() < 1.0);
    }

    #[test]
    fn test_gradient_positions() {
        let base = Hsl {
            h: 145.0,
            s: 0.63,
            l: 0.49,
        };
        let gradient = derive_gradient(base);

        // t=0 should be neutral
        let c0 = gradient_color(&gradient, 0.0);
        let h0 = rgb_to_hsl(c0);
        assert!(h0.s < 0.2); // low saturation

        // t=0.5 should be base
        let c5 = gradient_color(&gradient, 0.5);
        let h5 = rgb_to_hsl(c5);
        assert!((h5.s - base.s).abs() < 0.01);
        assert!((h5.l - base.l).abs() < 0.01);

        // t=1.0 should be bright
        let c1 = gradient_color(&gradient, 1.0);
        let h1 = rgb_to_hsl(c1);
        assert!(h1.l > base.l); // brighter
    }

    #[test]
    fn test_parse_ansi_name() {
        assert_eq!(parse_ansi_name("red"), Some(Color::Red));
        assert_eq!(parse_ansi_name("dark_gray"), Some(Color::DarkGray));
        assert_eq!(parse_ansi_name("darkgray"), Some(Color::DarkGray));
        assert_eq!(parse_ansi_name("dark-gray"), Some(Color::DarkGray));
        assert_eq!(parse_ansi_name("transparent"), Some(Color::Reset));
        assert_eq!(parse_ansi_name("unknown"), None);
    }

    #[test]
    fn test_resolve_color() {
        let mut defs = HashMap::new();
        defs.insert("oyo14".to_string(), "#A3BE8C".to_string());

        // Def reference
        assert!(matches!(
            resolve_color("oyo14", &defs),
            Some(Color::Rgb(163, 190, 140))
        ));

        // Hex
        assert!(matches!(
            resolve_color("#ff0000", &defs),
            Some(Color::Rgb(255, 0, 0))
        ));

        // ANSI
        assert_eq!(resolve_color("cyan", &defs), Some(Color::Cyan));

        // Default/reset for terminal palette
        assert_eq!(resolve_color("default", &defs), Some(Color::Reset));
        assert_eq!(resolve_color("reset", &defs), Some(Color::Reset));
        assert_eq!(resolve_color("transparent", &defs), Some(Color::Reset));

        // Unknown
        assert_eq!(resolve_color("notacolor", &defs), None);
    }

    #[test]
    fn test_dim_color() {
        // RGB color should be dimmed via HSL
        let bright = Color::Rgb(46, 204, 113);
        let dim = dim_color(bright);
        if let Color::Rgb(r, g, b) = dim {
            // Should be darker/less saturated
            let bright_rgb = Rgb {
                r: 46,
                g: 204,
                b: 113,
            };
            let dim_rgb = Rgb { r, g, b };
            let bright_hsl = rgb_to_hsl(bright_rgb);
            let dim_hsl = rgb_to_hsl(dim_rgb);
            assert!(dim_hsl.l < bright_hsl.l);
            assert!(dim_hsl.s < bright_hsl.s);
        } else {
            panic!("Expected RGB color");
        }

        // ANSI green should become dark_gray
        assert_eq!(dim_color(Color::Green), Color::DarkGray);
    }

    #[test]
    fn test_ease_out() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        // Midpoint should be > 0.5 (ease-out is front-loaded)
        assert!(ease_out(0.5) > 0.5);
    }

    #[test]
    fn test_lerp_rgb_color() {
        let black = Color::Rgb(0, 0, 0);
        let white = Color::Rgb(255, 255, 255);

        assert_eq!(lerp_rgb_color(black, white, 0.0), black);
        assert_eq!(lerp_rgb_color(black, white, 1.0), white);
        assert_eq!(lerp_rgb_color(black, white, 0.5), Color::Rgb(128, 128, 128));
    }
}
