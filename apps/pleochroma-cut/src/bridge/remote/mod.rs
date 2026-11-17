//! Everything specific to talking to a remote `pleochroma-worker`: claiming a one-time
//! enrollment token ([`enroll`]) or a pairing link ([`pair`]), the saved address list and
//! how the coordinator is verified ([`route`]), finding a moved coordinator again
//! ([`rediscover`]), owning the mutual-TLS socket and driving one render
//! request against it ([`remote_render`]), the preview-then-handoff state machine that
//! decides when a live viewport brings a remote worker in ([`handoff`]), the live
//! viewport's per-epoch remote chunk lane ([`live_lane`]), the one rule deciding
//! whether a remote worker may render a scene at all ([`guard`]), and loaded HDR maps as
//! content-addressed protocol assets ([`hdr_asset`]). Distinct from
//! [`super::render_thread`] (the local render loop) and [`super::export_thread`]'s own
//! export-specific remote dispatch.

pub mod enroll;
pub mod guard;
pub mod handoff;
pub mod hdr_asset;
pub mod live_lane;
pub mod pair;
pub mod rediscover;
pub mod remote_render;
pub mod route;

pub use guard::{
    LiveDispatch, inclusions_refusal, live_remote_dispatch, remote_can_render, zoned_scene_refusal,
};
