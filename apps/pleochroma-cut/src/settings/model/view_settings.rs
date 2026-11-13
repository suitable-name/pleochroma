//! Stored view-mode identifiers. Startup opens both views in Solid whatever is stored.
/// The solid preview's saved view-mode selector.
///
/// `0` Solid (the software-rasterized inspection view), `1` Path-Traced, `2` Solid +
/// Path-Traced, `3` Diagram (the faceting diagram), `4` Ray-Traced, `5` Solid +
/// Ray-Traced. Combined modes composite the solid's edges over the selected tracer.
/// A plain `u8` because a Slint `int` property round-trips it directly. The default
/// `0` keeps a settings file without this key on the Solid view.
pub const DEFAULT_SOLID_VIEW_MODE: u8 = 0;
/// The Live Render tab's Solid / Path-Traced view-mode selector.
///
/// `1` (Path-Traced) is the default, so a settings file without this key loads the
/// spectral-tracer view. Unlike [`DEFAULT_SOLID_VIEW_MODE`], this is not `0`. The modes
/// are listed on `ViewportModel.live_view_mode` in `ui/models/viewport.slint`.
pub const DEFAULT_LIVE_VIEW_MODE: u8 = 1;

pub(super) const fn default_live_view_mode() -> u8 {
    DEFAULT_LIVE_VIEW_MODE
}
