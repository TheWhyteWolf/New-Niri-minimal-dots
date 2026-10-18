// SPDX-License-Identifier: GPL-3.0-or-later
// Text on the glass (clock, date, prompts): lines rasterized with lifefont
// into one RGBA band, with a soft dark shadow so they stay readable over
// bright drops. The band is re-rendered only when its text changes (once a
// minute for the clock, plus state changes); the shader composites it.
//
// Rasterizing a run follows LifeBranch's lifelock (render/atlas.rs text_run).

pub use lifefont::Font;

pub struct Line {
    pub text: String,
    /// Glyph size in buffer pixels.
    pub px: f32,
    pub rgb: [u8; 3],
    /// 0..1, multiplied into the coverage.
    pub alpha: f32,
}

/// A block of text to lay over the output, positioned in buffer pixels.
pub struct Band {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
    /// Straight (not premultiplied) RGBA, top-down.
    pub rgba: Vec<u8>,
}

struct Run {
    w: usize,
    h: usize,
    cov: Vec<u8>,
}

fn run(font: &Font, text: &str, px: f32) -> Option<Run> {
    let mut pen = 0i32;
    let (mut top, mut bot) = (i32::MAX, i32::MIN);
    let mut glyphs = Vec::new();
    for ch in text.chars() {
        let (m, cov) = font.rasterize(ch, px);
        // lifefont measures y up from the baseline; ymin is the bottom edge.
        let y_top = -(m.ymin + m.height as i32);
        top = top.min(y_top);
        bot = bot.max(y_top + m.height as i32);
        glyphs.push((pen + m.xmin, y_top, m.width, m.height, cov));
        pen += m.advance_width.round() as i32;
    }
    if glyphs.is_empty() || top > bot {
        return None;
    }
    let x_off = glyphs.iter().map(|g| g.0).min().unwrap_or(0);
    let (w, h) = ((pen - x_off).max(1) as usize, (bot - top).max(1) as usize);
    let mut cov = vec![0u8; w * h];
    for (gx, gy, gw, gh, gcov) in glyphs {
        for ry in 0..gh {
            for rx in 0..gw {
                let c = gcov[ry * gw + rx];
                let (x, y) = ((gx - x_off) as usize + rx, (gy - top) as usize + ry);
                if c > 0 && x < w && y < h {
                    let d = &mut cov[y * w + x];
                    *d = (*d).max(c);
                }
            }
        }
    }
    Some(Run { w, h, cov })
}

/// Lay `lines` out centred on an output `screen_w` pixels wide, the first
/// line's top at `top`, `gap` pixels between lines. Empty lines still take
/// their height, so what follows them doesn't jump. None when there is no
/// text at all.
pub fn band(font: &Font, lines: &[Line], screen_w: usize, top: i32, gap: usize) -> Option<Band> {
    let runs: Vec<Option<Run>> = lines.iter().map(|l| run(font, &l.text, l.px)).collect();
    if runs.iter().all(Option::is_none) {
        return None;
    }
    let heights: Vec<usize> = lines.iter().zip(&runs).map(|(l, r)| r.as_ref().map_or(l.px as usize, |r| r.h)).collect();
    // Room for the shadow to spread past the glyphs.
    let pad = (lines.iter().map(|l| l.px).fold(0.0, f32::max) * 0.12).ceil() as usize + 2;
    let text_w = runs.iter().flatten().map(|r| r.w).max().unwrap_or(1);
    let w = text_w + 2 * pad;
    let h = heights.iter().sum::<usize>() + gap * lines.len().saturating_sub(1) + 2 * pad;

    // Coverage and colour per pixel, then the shadow from the coverage.
    let mut cov = vec![0f32; w * h];
    let mut rgb = vec![[0u8; 3]; w * h];
    let mut y = pad;
    for ((line, r), lh) in lines.iter().zip(&runs).zip(&heights) {
        if let Some(r) = r {
            let x0 = (w - r.w) / 2;
            for ry in 0..r.h {
                for rx in 0..r.w {
                    let c = r.cov[ry * r.w + rx] as f32 / 255.0 * line.alpha;
                    let i = (y + ry) * w + x0 + rx;
                    if c > cov[i] {
                        cov[i] = c;
                        rgb[i] = line.rgb;
                    }
                }
            }
        }
        y += lh + gap;
    }
    let mut shadow = cov.clone();
    for _ in 0..2 {
        blur(&mut shadow, w, h, pad / 2 + 1);
    }

    let mut rgba = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let (t, s) = (cov[i], (shadow[i] * 0.75).min(1.0));
        // Text over its shadow (black), straight alpha.
        let a = t + s * (1.0 - t);
        if a <= 0.0 {
            continue;
        }
        for c in 0..3 {
            rgba[i * 4 + c] = (rgb[i][c] as f32 * t / a).round() as u8;
        }
        rgba[i * 4 + 3] = (a * 255.0).round() as u8;
    }
    Some(Band { x: (screen_w as i32 - w as i32) / 2, y: top - pad as i32, w, h, rgba })
}

/// Separable box blur on a coverage map, in place. Edges clamp.
fn blur(v: &mut [f32], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0f32; v.len()];
    let win = (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for k in 0..=2 * r {
                s += v[y * w + (x + k).saturating_sub(r).min(w - 1)];
            }
            tmp[y * w + x] = s / win;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for k in 0..=2 * r {
                s += tmp[(y + k).saturating_sub(r).min(h - 1) * w + x];
            }
            v[y * w + x] = s / win;
        }
    }
}
