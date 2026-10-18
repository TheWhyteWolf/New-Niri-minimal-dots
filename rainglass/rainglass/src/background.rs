// SPDX-License-Identifier: GPL-3.0-or-later
// The picture behind the glass: the wallpaper cover-fitted to one output, and
// a frosted copy of it. Both are made once per output size on the CPU; the
// shader then mixes between them per pixel (sharp through drops and wiped
// trails, frosted everywhere else).
//
// The frosted copy is a quarter-resolution image blurred with three box-blur
// passes (close to a Gaussian, sigma ~6.5 px there, ~26 px on screen). Linear
// filtering when it is stretched back up hides the low resolution, and it
// keeps the blur cheap: ~130k pixels for a 1080p output.

use image::imageops::FilterType;
use image::{DynamicImage, RgbImage};

pub struct Background {
    pub w: usize,
    pub h: usize,
    /// RGB, w*h, top-down.
    pub sharp: Vec<u8>,
    pub frost_w: usize,
    pub frost_h: usize,
    /// RGB, frost_w*frost_h, top-down.
    pub frost: Vec<u8>,
}

/// How much smaller the frosted copy is than the output.
const FROST_DIV: u32 = 4;
const FROST_RADIUS: usize = 6;
const FROST_PASSES: usize = 3;

/// A decoded wallpaper, ready to be fitted to any output.
pub struct Source(DynamicImage);

/// Decode `path` once and shrink it, keeping its shape, until it only just
/// covers `max_w`x`max_h`. Fitting it to each output later starts from this
/// instead of from the original: a 6600x3600 photo is 71 MB decoded, which a
/// lock screen shouldn't keep around, and decoding is the slow part.
pub fn load_source(path: &str, max_w: u32, max_h: u32) -> Result<Source, String> {
    let img = image::open(path).map_err(|e| format!("{path}: {e}"))?;
    let s =
        (max_w as f64 / img.width().max(1) as f64).max(max_h as f64 / img.height().max(1) as f64);
    if s >= 1.0 {
        return Ok(Source(DynamicImage::ImageRgb8(img.to_rgb8())));
    }
    let (w, h) = (
        (img.width() as f64 * s).ceil() as u32,
        (img.height() as f64 * s).ceil() as u32,
    );
    Ok(Source(DynamicImage::ImageRgb8(image::imageops::resize(
        &img.to_rgb8(),
        w.max(1),
        h.max(1),
        FilterType::Triangle,
    ))))
}

impl Background {
    /// Load `path` and fit it to `w`x`h` buffer pixels.
    pub fn load(path: &str, w: u32, h: u32) -> Result<Background, String> {
        let img = image::open(path).map_err(|e| format!("{path}: {e}"))?;
        Ok(Background::from_image(&img, w, h))
    }

    /// Fit a decoded wallpaper to `w`x`h` buffer pixels.
    pub fn from_source(src: &Source, w: u32, h: u32) -> Background {
        Background::from_image(&src.0, w, h)
    }

    /// A plain colour, for when there is no wallpaper to show.
    pub fn solid(rgb: [u8; 3], w: u32, h: u32) -> Background {
        let img =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(w.max(1), h.max(1), image::Rgb(rgb)));
        Background::from_image(&img, w, h)
    }

    pub fn from_image(img: &DynamicImage, w: u32, h: u32) -> Background {
        let (w, h) = (w.max(1), h.max(1));
        let sharp = cover(img, w, h);
        let (fw, fh) = ((w / FROST_DIV).max(1), (h / FROST_DIV).max(1));
        let mut frost = image::imageops::resize(&sharp, fw, fh, FilterType::Triangle).into_raw();
        for _ in 0..FROST_PASSES {
            box_blur(&mut frost, fw as usize, fh as usize, FROST_RADIUS);
        }
        Background {
            w: w as usize,
            h: h as usize,
            sharp: sharp.into_raw(),
            frost_w: fw as usize,
            frost_h: fh as usize,
            frost,
        }
    }
}

/// Scale to cover `w`x`h` exactly, cropping the overflow equally from both
/// sides (what swaybg's `fill` mode does).
fn cover(img: &DynamicImage, w: u32, h: u32) -> RgbImage {
    let (iw, ih) = (img.width().max(1) as f64, img.height().max(1) as f64);
    let s = (w as f64 / iw).max(h as f64 / ih);
    // Crop in source pixels first: resizing a 6600px photo whole only to
    // throw half of it away is most of the cost.
    let (cw, ch) = (
        (w as f64 / s).round().min(iw) as u32,
        (h as f64 / s).round().min(ih) as u32,
    );
    let (cx, cy) = (
        (img.width() - cw.min(img.width())) / 2,
        (img.height() - ch.min(img.height())) / 2,
    );
    let cropped = img.crop_imm(cx, cy, cw.max(1), ch.max(1)).to_rgb8();
    image::imageops::resize(&cropped, w, h, FilterType::Triangle)
}

/// One separable box-blur pass over RGB pixels, in place. Edges clamp.
fn box_blur(px: &mut [u8], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0u8; px.len()];
    blur_1d(px, &mut tmp, w, h, r, true);
    blur_1d(&tmp, px, w, h, r, false);
}

fn blur_1d(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| {
        if horizontal {
            (line * w + i) * 3
        } else {
            (i * w + line) * 3
        }
    };
    let win = (2 * r + 1) as u32;
    for line in 0..lines {
        let mut sum = [0u32; 3];
        // Window centred on i = 0, edge pixel repeated.
        for k in 0..=2 * r {
            let i = k.saturating_sub(r).min(len - 1);
            for c in 0..3 {
                sum[c] += src[at(line, i) + c] as u32;
            }
        }
        for i in 0..len {
            for c in 0..3 {
                dst[at(line, i) + c] = ((sum[c] + win / 2) / win) as u8;
            }
            let out = i.saturating_sub(r);
            let inn = (i + r + 1).min(len - 1);
            for c in 0..3 {
                sum[c] = sum[c] + src[at(line, inn) + c] as u32 - src[at(line, out) + c] as u32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_keeps_a_flat_image_flat() {
        let mut px = vec![100u8; 9 * 7 * 3];
        box_blur(&mut px, 9, 7, 3);
        assert!(px.iter().all(|&v| v == 100));
    }

    #[test]
    fn blur_spreads_a_bright_pixel() {
        let (w, h) = (11, 11);
        let mut px = vec![0u8; w * h * 3];
        px[(5 * w + 5) * 3] = 255;
        box_blur(&mut px, w, h, 1);
        assert!(px[(5 * w + 5) * 3] < 255);
        assert!(px[(5 * w + 6) * 3] > 0 && px[(6 * w + 5) * 3] > 0);
    }

    #[test]
    fn cover_fills_the_target_exactly() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(300, 100, image::Rgb([1, 2, 3])));
        let bg = Background::from_image(&img, 64, 64);
        assert_eq!((bg.w, bg.h), (64, 64));
        assert_eq!(bg.sharp.len(), 64 * 64 * 3);
        assert_eq!((bg.frost_w, bg.frost_h), (16, 16));
    }
}
