// SPDX-License-Identifier: GPL-3.0-or-later
// Fractional scaling (wp-fractional-scale-v1 + wp-viewporter).
//
// wl_output only knows integer scales, so at 1.25 a client would have to draw
// at 2x and let the compositor shrink it: 2.56 times the pixels the panel
// has, every frame, for a full-screen shader. With these two protocols the
// compositor says the real scale (in 120ths) and the client draws a buffer of
// exactly the output's pixel size, shown at the logical size through a
// viewport. Without them, callers fall back to the integer buffer scale.
//
// Use: `Scaling::bind` once, `attach` per surface, implement ScaleHandler on
// the app, and `rainglass::delegate_scaling!(App);`.

use smithay_client_toolkit::reexports::client::{
    globals::GlobalList, protocol::wl_surface::WlSurface, Connection, Dispatch, QueueHandle,
};
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

pub trait ScaleHandler {
    /// The compositor's preferred scale for `surface`, in 120ths (150 = 1.25).
    fn preferred_scale(&mut self, surface: &WlSurface, scale120: u32);
}

pub struct Scaling {
    manager: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
}

/// One surface's fractional-scale object and viewport.
pub struct SurfaceScale {
    fractional: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
}

impl Scaling {
    pub fn bind<D>(globals: &GlobalList, qh: &QueueHandle<D>) -> Scaling
    where
        D: Dispatch<WpFractionalScaleManagerV1, ()> + Dispatch<WpViewporter, ()> + 'static,
    {
        Scaling {
            manager: globals.bind(qh, 1..=1, ()).ok(),
            viewporter: globals.bind(qh, 1..=1, ()).ok(),
        }
    }

    pub fn attach<D>(&self, surface: &WlSurface, qh: &QueueHandle<D>) -> SurfaceScale
    where
        D: Dispatch<WpFractionalScaleV1, WlSurface> + Dispatch<WpViewport, ()> + 'static,
    {
        match (&self.manager, &self.viewporter) {
            (Some(m), Some(v)) => SurfaceScale {
                fractional: Some(m.get_fractional_scale(surface, qh, surface.clone())),
                viewport: Some(v.get_viewport(surface, qh, ())),
            },
            _ => SurfaceScale {
                fractional: None,
                viewport: None,
            },
        }
    }
}

impl SurfaceScale {
    /// Whether buffers can be any size (shown through the viewport).
    pub fn fractional(&self) -> bool {
        self.viewport.is_some()
    }

    /// Show whatever buffer is attached at `w`x`h` logical pixels.
    pub fn set_destination(&self, w: u32, h: u32) {
        if let Some(v) = &self.viewport {
            v.set_destination(w as i32, h as i32);
        }
    }
}

impl Drop for SurfaceScale {
    fn drop(&mut self) {
        if let Some(v) = self.viewport.take() {
            v.destroy();
        }
        if let Some(f) = self.fractional.take() {
            f.destroy();
        }
    }
}

/// Buffer pixels for `logical` pixels at `scale120`, rounded as the protocol
/// specifies (half away from zero).
pub fn buffer_px(logical: u32, scale120: u32) -> u32 {
    ((logical as u64 * scale120 as u64 + 60) / 120).max(1) as u32
}

impl<D> Dispatch<WpFractionalScaleV1, WlSurface, D> for Scaling
where
    D: Dispatch<WpFractionalScaleV1, WlSurface> + ScaleHandler,
{
    fn event(
        state: &mut D,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &WlSurface,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.preferred_scale(surface, scale);
        }
    }
}

// The manager, viewporter and viewport have no events.
impl<D> Dispatch<WpFractionalScaleManagerV1, (), D> for Scaling
where
    D: Dispatch<WpFractionalScaleManagerV1, ()>,
{
    fn event(
        _: &mut D,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D> Dispatch<WpViewporter, (), D> for Scaling
where
    D: Dispatch<WpViewporter, ()>,
{
    fn event(
        _: &mut D,
        _: &WpViewporter,
        _: <WpViewporter as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D> Dispatch<WpViewport, (), D> for Scaling
where
    D: Dispatch<WpViewport, ()>,
{
    fn event(
        _: &mut D,
        _: &WpViewport,
        _: <WpViewport as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

/// Route the four protocol objects' events for `$app` through [`Scaling`].
#[macro_export]
macro_rules! delegate_scaling {
    ($app:ty) => {
        $crate::sctk::reexports::client::delegate_dispatch!($app:
            [$crate::sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1: ()] => $crate::scale::Scaling);
        $crate::sctk::reexports::client::delegate_dispatch!($app:
            [$crate::sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1: $crate::sctk::reexports::client::protocol::wl_surface::WlSurface] => $crate::scale::Scaling);
        $crate::sctk::reexports::client::delegate_dispatch!($app:
            [$crate::sctk::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter: ()] => $crate::scale::Scaling);
        $crate::sctk::reexports::client::delegate_dispatch!($app:
            [$crate::sctk::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport: ()] => $crate::scale::Scaling);
    };
}

#[cfg(test)]
mod tests {
    use super::buffer_px;

    #[test]
    fn buffer_size_at_common_scales() {
        assert_eq!(buffer_px(1536, 150), 1920); // 1.25: this laptop
        assert_eq!(buffer_px(864, 150), 1080);
        assert_eq!(buffer_px(1280, 180), 1920); // 1.5
        assert_eq!(buffer_px(1920, 120), 1920); // 1.0
        assert_eq!(buffer_px(1, 150), 1); // 1.25 rounds half up
    }
}
