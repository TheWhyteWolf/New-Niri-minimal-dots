// SPDX-License-Identifier: GPL-3.0-or-later
// The fallback when there is no GPU to draw with (EGL failed to start, or a
// surface couldn't get one): the frosted wallpaper and the text, drawn once on
// the CPU into a shm buffer. No drops, no animation — but the screen is
// covered and the password prompt works exactly the same.

use crate::{Background, Band};

/// Fill `canvas` (XRGB8888, `w`x`h`, little-endian B,G,R,X) with the frosted
/// background stretched to fit, dimmed, with `band` composited over it.
pub fn paint(canvas: &mut [u8], w: usize, h: usize, bg: &Background, band: Option<&Band>, dim: f32) {
    let (fw, fh) = (bg.frost_w, bg.frost_h);
    let axis = |i: usize, n: usize, src: usize| {
        let f = ((i as f32 + 0.5) * src as f32 / n as f32 - 0.5).clamp(0.0, (src - 1) as f32);
        let a = f.floor() as usize;
        (a, (a + 1).min(src - 1), f - a as f32)
    };
    for y in 0..h {
        let (y0, y1, ty) = axis(y, h, fh);
        for x in 0..w {
            let (x0, x1, tx) = axis(x, w, fw);
            let at = |xx: usize, yy: usize, c: usize| bg.frost[(yy * fw + xx) * 3 + c] as f32;
            let px = &mut canvas[(y * w + x) * 4..][..4];
            for c in 0..3 {
                let top = at(x0, y0, c) * (1.0 - tx) + at(x1, y0, c) * tx;
                let bot = at(x0, y1, c) * (1.0 - tx) + at(x1, y1, c) * tx;
                px[2 - c] = ((top * (1.0 - ty) + bot * ty) * dim).round() as u8;
            }
            px[3] = 255;
        }
    }
    let Some(b) = band else { return };
    for by in 0..b.h {
        let y = b.y + by as i32;
        if y < 0 || y as usize >= h {
            continue;
        }
        for bx in 0..b.w {
            let x = b.x + bx as i32;
            if x < 0 || x as usize >= w {
                continue;
            }
            let src = &b.rgba[(by * b.w + bx) * 4..][..4];
            let a = src[3] as f32 / 255.0;
            if a == 0.0 {
                continue;
            }
            let px = &mut canvas[(y as usize * w + x as usize) * 4..][..4];
            for c in 0..3 {
                px[2 - c] = (src[c] as f32 * a + px[2 - c] as f32 * (1.0 - a)).round() as u8;
            }
        }
    }
}
