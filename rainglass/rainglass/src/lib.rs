// SPDX-License-Identifier: GPL-3.0-or-later
//! rainglass: the wallpaper seen through a rainy window. The renderer shared
//! by rainlock (the lock screen) and raingreet (the login screen).
//!
//! * [`background`]: the wallpaper fitted to an output, plus a frosted copy.
//! * [`rain`]: the drop shader, per-output textures, per-frame state.
//! * [`text`]: clock and prompts, rasterized with lifefont.
//! * [`scale`]: fractional scaling, so buffers match the panel's pixels.
//! * [`still`]: one still frame on the CPU, for when there is no GPU.
//! * [`gpu`]: EGL + GLES2, from LifeBranch's lifewall.

pub mod background;
pub mod gpu;
pub mod rain;
pub mod scale;
pub mod still;
pub mod text;

pub use background::{load_source, Background, Source};
pub use gpu::{Gpu, Target};
pub use rain::{Frame, Impact, Plate, Rain, IMPACTS};
pub use scale::{buffer_px, ScaleHandler, Scaling, SurfaceScale};
pub use text::{Band, Font, Line};

/// For `delegate_scaling!` and for callers that want the same pinned version.
pub use smithay_client_toolkit as sctk;
