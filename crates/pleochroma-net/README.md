# pleochroma-net

Wire protocol between a viewer and a remote server — and between that server (a
coordinator) and the render workers that join it: **offloading `pleochroma` spectral
ray-sample computation**, and **reading a remote design library**. Both run over one
authenticated connection, and a server may offer either or both — `WELCOME` says
which.

**Types, codec, and framing, plus one small client.** Every function in this crate
operates on an in-memory buffer, a `[u8]` slice, or a generic `Read`/`Write` — with the
single exception of `enroll::claim`, which opens a `TcpStream` to a worker's enrollment
listener (with connect and read timeouts). The protocol itself is testable with nothing
more than a `std::io::Cursor`, and its tests do exactly that. `apps/pleochroma-worker`
(the coordinator, and a `join`ed worker's side of its connection) and
`apps/pleochroma-cut`'s `bridge::remote::remote_render` / `bridge::library::client`
(the viewer side) are what wire these functions to a real `TcpStream`/TLS connection.

| Module | What it carries |
|---|---|
| `messages` | The render protocol, `HELLO`/`WELCOME` (with peer roles), the `ClientMessage` envelope, payload-encoding negotiation, and content-addressed assets (`NEED_ASSET`/`ASSET`) |
| `library` | The read-only design-library protocol |
| `scene` | `SceneState` — a fully-resolved scene (`render` feature) |
| `radiance` | The raw radiance-buffer encoding and the lossless payload encodings (shuffle + zstd/LZ4) |
| `display` | The 8-bit picture payloads (raw RGBA8 or PNG) |
| `framing` / `handshake` | Length-prefixed framing; the version and build-hash gate |
| `tls` | Mutual-TLS config, the fingerprint allowlist, restricted-permission key writes |
| `enroll` / `token` | One-time enrollment tokens and the CA-pinning claim client |
| `client` | Helpers for driving a connection from the viewer's side |

## Why the split works

`pleochroma::optics::raytracer::trace_spectral_ray` returns one sample's XYZ radiance,
and a viewer's accumulation buffer is just `Vec<Vec3>` of running per-pixel sums.
Samples are additive and order-independent, so a remote node's contribution is
nothing more than more terms in that same sum — there is no partial-frame
reassembly or per-tile stitching to get right.

Work is partitioned **by sample index** (every node traces a disjoint range of
sample numbers across the *whole* frame), not by screen-space tile: a gem only
occupies part of the frame and background pixels are nearly free to trace, so tile
partitioning would load-balance badly, while sample partitioning divides the work
evenly by construction. This only works because the per-sample RNG seed is a pure
function of `(pixel_index, sample_number)` — never of which batch a sample happens
to land in, or how many samples are in that batch. Additivity against the real tracer
is checked in `apps/pleochroma-worker`'s `render_core` and `live_split` tests: tracing
samples `[0,64)` in one batch sums to (within float-rounding tolerance) the same
radiance as tracing `[0,32)` and `[32,64)` separately and adding the results.

**Samples are summed, never averaged**, anywhere in this crate or its wire
format. Normalizing a sum into a displayable average (dividing by total sample
count) is a client-side, display-time operation — the wire payload is always a raw
radiance sum, which is what makes it valid to keep adding more nodes' contributions
into the same buffer without knowing in advance how many total samples there will
be. Every backend applies the same non-finite rule while summing
(`pleochroma::optics::raytracer::add_finite_sample` and its GPU twin): a sample with
any NaN/±Inf component is dropped but still counted, so sums from different nodes
stay mergeable by plain addition. The receiving side applies the same rule to what a
peer sends (a pixel with a non-finite or negative component is skipped and counted, and
the frame's samples still count as done), because a dishonest producer is not bound by
it.

## Protocol version and message set

`messages::PROTOCOL_VERSION: u16 = 44` (see its doc comment for the history). Three protocols share one authenticated
connection: **render** (offload sample tracing), **tilt curves** (offload one design's
full tilt-performance sweep), and **library** (read a design catalogue). A peer may
serve any subset, and — for a viewer — consume all three.

**Beam tiles.** `BEAM_TILE_REQUEST` and `BEAM_TILES` are part of every `render` build so
that indices and decoding agree between builds; only builds with the apps' retired `beam`
features send or serve them; a worker or coordinator built without the feature answers
`UNSUPPORTED_REQUEST` (code 5) and keeps the connection.

**Live passes.** `LIVE_PASS_REQUEST` and `LIVE_PASS` stay in every `render` build for the
same reason. Adaptive sampling is an optional build feature of the apps (`adaptive-sampling`); a
worker or coordinator built without it answers a `LivePassRequest` with `UNSUPPORTED_REQUEST`
(code 5) and keeps the connection, and a viewer treats that as a refusal.

```text
-> HELLO    { protocol_version, build_hash, source_hash, role: Viewer | Worker,
              capability: Option<RenderCapability>, accept_encodings: Vec<PayloadEncoding> }
<- WELCOME  { protocol_version, build_hash, source_hash, render: Option<RenderCapability>, library: bool,
              tilt_curves: bool, registration: Option<WorkerRegistration>, payload_encoding }
                                  RenderCapability { backend: Cpu | Gpu | Coordinator, max_pixels, min_cadence_ms, hdr }
```

**Roles.** `HELLO.role` says who is dialing. A `Viewer` (the GUI) sends requests. A
`Worker` is `pleochroma-worker join` dialing *out* to a coordinator's worker port: its
`HELLO` carries its `RenderCapability`, the coordinator answers with
`WELCOME.registration = Some { worker_id }`, and from then on the roles on that
connection are reversed — the coordinator sends requests (and `PING`s), the worker
answers them. A listener refuses a `HELLO` of the wrong role with `ROLE_REFUSED`
(error codes: `messages::error_codes`).

The first three fields of `HELLO` and `WELCOME` never change, so a peer on another
protocol version is refused with a message naming both versions
(`handshake::read_hello`, and the client's own check of the reply) rather than a
decode error. **Every machine must run the same build.**

**Payload encodings.** The viewer lists what it can decode
(`accept_encodings`); the server picks the first entry of its own preference the viewer
accepts (`messages::negotiate`), else `Raw`. Default server preference:
`ShuffleZstd { level: 1 }`, `ShuffleLz4`, `Raw`; loopback peers get `Raw`. Every
`FRAME`/`PREVIEW` header names its own `encoding` and `raw_len` (`payload_len` is the
on-wire size), and a payload that would not shrink goes out `Raw`. Decoding is bounded:
`raw_len` must equal `width * height * 12`, output goes into exactly `raw_len` bytes,
short or overflowing output is rejected (`radiance::payload`). All encodings are
lossless: a compressed delta sums bit-identically to the raw one. The compressed
encodings and the PNG display codec sit behind the default-on `compression` feature.

`WELCOME` is where a peer says what it can actually do. **Check it before sending
anything**: `render` is `None` on a library-only server (one built without
`pleochroma-worker`'s `worker` feature), `tilt_curves` is `false` on that same server (it
tracks `render.is_some()` -- see `messages::Welcome::tilt_curves`'s own
doc comment), and `library` is `false` on a server that only renders. Discovering the
absence by having a request rejected is the wrong shape — `bridge::remote_render`'s
`NoRenderCapacity` and `bridge::library_client`'s `NoLibraryCapacity` are the
client-side counterparts.

**Render** (gated behind this crate's `render` feature — see below):

```text
-> RENDER   { request_id, scene: SceneState, first_sample, samples, stream: StreamConfig, intent }
<- FRAME    { request_id, first_sample, samples, payload_len, encoding, raw_len } + payload        -- DELTA, full-res
<- PREVIEW  { request_id, width, height, samples_done, payload_len, encoding, raw_len } + payload -- CUMULATIVE, reduced-res
<- PROGRESS { request_id, samples_done }
-> CANCEL   { request_id }
<- DONE     { request_id, cancelled, stats: Stats }             -- Stats.reclaimed_samples counts reclaimed samples
<- ERROR    { code, message }                                 -- codes: messages::error_codes
-> PING     { nonce }                    <- PONG { nonce }     -- liveness
-> FINAL_IMAGE_REQUEST { request_id, scene, first_sample, samples, width, height, color_space, output,
                          viewer_samples }
<- FINAL_IMAGE   { request_id, width, height, samples_done, encoding, payload_len } + PNG
<- DISPLAY_FRAME { request_id, samples_done, width, height, encoding, payload_len } + RGBA8/PNG (TransferMode::DisplayOnly)
<- CAPABILITY_CHANGED { render: Option<RenderCapability> }
<- NEED_ASSET { content_hash }                               -- "send me this HDR map"
-> ASSET      { content_hash, len } + bytes                    -- the map's exact file bytes
-> CONTRIBUTION { request_id, first_sample, samples, width, height, encoding, payload_len, raw_len } + payload
                                                                 -- the viewer's own share of a
                                                                    FINAL_IMAGE_REQUEST's reserved tail
```

A `FRAME`'s `samples` is exact and `[first_sample, first_sample + samples)` lies inside
the request range; a coordinator's frame carries a *set* of samples from several lanes
with `first_sample = request.first_sample`, so clients check containment, never
contiguity. `FINAL_IMAGE_REQUEST` and `TransferMode::DisplayOnly` are served by the
coordinator (`pleochroma-worker serve`, with or without joined workers); a joined
worker's own connection refuses them with `UNSUPPORTED_REQUEST`, since the coordinator
only ever sends it radiance chunks. `DISPLAY_FRAME`s are averaged, denoised
(`pleochroma::renderer::frame_denoise`) and tone-mapped exactly as the viewer would
have done it itself.

**HDR environments.** `SceneState::environment` is either the analytic studio
rig or `SceneEnvironment::Hdr { content_hash, width, height }` — an HDR panorama named
by the SHA-256 of its `.hdr` file, never by path. A server that lacks the bytes answers
the request with `NEED_ASSET`; the client sends `ASSET` (at most
`messages::MAX_ASSET_LEN`, 256 MiB), the server verifies the hash and decodes the map
with the same builder the viewer uses. A viewer sends an HDR scene only to a server
whose `RenderCapability::hdr` is `true`.

**Tilt curves** (gated behind this crate's `render` feature, same as render — see
below): a single request, a single reply, never a `StreamEvent` stream. One design's
full brilliance/extinction/windowing sweep across 4 axes x 181 tilt points (8,688 bytes,
measured ~1.36s to compute) is small and fast enough that `RENDER`'s progressive
delta/preview/cadence machinery buys nothing — see `messages::tilt`'s module doc
comment.

```text
-> TILT_CURVES  { request_id, scene: SceneState }         -- only scene.planes/material/light_yaw/light_pitch matter
<- <TiltCurvesResponse>
     Curves    { request_id, axes: [AxisTiltCurves; 4] }  -- brilliance_pct/extinction_pct/windowing_pct: Vec<f32>, 181 points each
     Cancelled { request_id }
     Error     { code, message }                          -- same ErrorMsg shape and codes RENDER's own validation/panic paths use
```

**Library** (always available, `library::LibraryRequest`/`LibraryResponse`):

```text
-> Search       { filters }              <- SearchResults(Vec<DesignSummary>)
-> SearchPage   { filters, after_id }    <- SearchResultsPage { summaries, next_after_id }
-> FilterOptions                          <- FilterOptions { shapes, gears, ranges }
-> FetchDesign  { entry_id }             <- Design(DesignRecord)   -- metadata, not bytes
-> FetchAttachment { attachment_id }     <- Attachment { bytes }
                                          <- NotFound | Error
```

`SearchPage` is the one to use for anything that must see the whole catalogue:
`Search` caps at 1000 rows, so a mirror built on it silently stops there. Paging is
a **keyset cursor** (`after_id`), not an offset — `diagram_entries.id` is
`AUTOINCREMENT`, so `ORDER BY id ASC` is a total order and a walk cannot skip or
duplicate rows when something is inserted mid-sync.

Both protocols run over one connection and one handshake. A client doing many
requests should hold the stream open rather than reconnecting per request — a full
mutual-TLS handshake per `FetchDesign` turns a 3,000-design mirror into 3,000
handshakes.

### The `render` feature

`pleochroma-net` depends on `pleochroma` **only** behind its `render` feature, because
`SceneState`, `RenderRequest`, and `TiltCurvesRequest`/`TiltCurvesResponse` all need
`pleochroma`'s resolved scene and material types (or, for the response, `pleochroma`'s tilt-curve
sweep shape). Off by default, so a library-only `pleochroma-worker` build never compiles the
renderer in at all. A viewer enables it — it is a render client.

One consequence worth knowing: `ClientMessage::RenderRequest` and
`ClientMessage::TiltCurvesRequest` exist only under that feature, and it is a *different
crate's* flag from `pleochroma-worker`'s `worker`. Cargo unifies features across a workspace
build, so a library-only worker can be compiled with either variant in scope.
`pleochroma-worker`'s dispatch handles that as a runtime case rather than a compile-time
impossibility — a peer can always send one regardless of what `WELCOME` advertised (see
`apps/pleochroma-worker/src/serve/connection/mod.rs`'s `handle_non_render_message` for exactly
how, and why it can only answer both with the same `StreamEvent::Error` shape in that
one narrow fallback path).

### When the version must be bumped

**This is pre-release software: every peer runs its own private build, so there is no
deployed fleet to stay wire-compatible with.** There is no compatibility shim between
versions — `handshake::verify_compatible` simply refuses to pair peers speaking
different ones. The changes made in each version are listed in `messages::PROTOCOL_VERSION`'s
doc comment; the rules for bumping are these.

Appending a field shifts the byte layout of every message that embeds it, so the bump turns a
silent misread into a clear handshake refusal (the build hash would refuse a peer on another
build anyway, but the version names the cause). The rules follow from postcard being
**not self-describing**:

- **Enums are encoded by declaration-order index, not by name.** Appending a variant
  leaves every existing message byte-identical, but a peer that predates the new
  variant sees an unrecognized index and fails with an opaque per-message decode
  error. It cannot skip what it does not recognize.
- **Structs are encoded as their fields in declaration order**, with no field names
  and no length prefix. Appending a field is worse than an unknown variant: the other
  peer silently *misaligns* every field after it instead of failing. This applies
  transitively — a field appended to anything embedded in `RenderRequest`, such as
  `SceneState` or the `GemMaterial` inside it, changes the layout of every message
  carrying it.

So append a variant to a wire enum, or a field to a wire struct or anything it
embeds, and bump. The bump is what converts an opaque decode failure (or worse,
silently misread data) into a diagnosable refusal at the handshake.

`#[serde(default)]` is not a substitute. It matters for self-describing formats —
`pleochroma-worker`'s local `scene.json` files, which never cross this protocol — and does
nothing at all for postcard's fixed-layout encoding.

### Tagged envelopes, and why they exist

A fixed reply shape — one `RENDER` producing exactly one `FRAME` or `ERROR` — would
need no tag at all. This protocol does: one `RENDER` produces a variable-length,
*interleaved* sequence of `FRAME`/`PREVIEW`/`PROGRESS` messages before a terminal
`DONE`, and a trace panic can happen mid-stream, after some of those have already
gone out, not only as the very first reply. Without a tag, a reader has no way to
tell "the next message is a `FRAME`" from "the next message is actually an `ERROR`,
because tracing just panicked after streaming three frames already" other than
guessing.
`messages::StreamEvent` is that tag for the reply direction.

The request direction has the same problem once a client is allowed to pipeline its
next `RenderRequest` without first waiting for `DONE` on the one currently
streaming: whatever bytes arrive next could be either a `CANCEL` for the in-flight
request or the next `RenderRequest`. `messages::ClientMessage` is the mirror-image
tag for that direction (`RenderRequest` is boxed inside it purely to keep the
enum's stack footprint small, since most `ClientMessage`s are `Cancel`).

## `FRAME` (delta) vs `PREVIEW` (cumulative) — read this before touching either

This distinction is load-bearing for correctness, not a naming choice:

- **`FRAME` carries a delta** — the summed contribution of exactly the sample
  sub-range named by `FrameHeader::first_sample`/`FrameHeader::samples`, at full
  resolution. A client's `Accumulator` **sums** every `FRAME` it receives (whose
  `request_id` matches its current epoch) straight into its running total. This is
  what makes deltas coalesce losslessly under backpressure: two adjacent,
  not-yet-sent deltas sum to one delta over their union, and the sum is identical
  whether it went out as one `FRAME` or several.
- **`PREVIEW` carries a cumulative, reduced-resolution snapshot** — the *full*
  running total so far (not a delta), downsampled to `PreviewHeader::width` x
  `PreviewHeader::height`. Each `PREVIEW` **replaces** the previous one; a client
  never sums two `PREVIEW`s together, and never sums a `PREVIEW` into the
  full-resolution accumulator — a reduced-resolution buffer is not additive with a
  full-resolution one under any arithmetic. This is also what makes `PREVIEW`
  freely droppable under backpressure: losing one just means the next one (which
  already reflects everything the dropped one did, plus more) arrives instead.

`client::accumulate::Accumulator::apply` is the one place this rule is
implemented: a `StreamEvent::Frame` is summed elementwise into `buffer`; a
`StreamEvent::Preview` outright replaces `last_preview`. A test
(`preview_snapshots_replace_rather_than_sum_and_never_touch_the_frame_buffer`)
sends preview values 10.0 then 20.0 and asserts the stored preview is `20.0`, not
`30.0`, and that a separately-summed `FRAME` buffer is untouched by either.

## `request_id` and cancellation epochs

Every message from `RENDER` onward — `FrameHeader`, `PreviewHeader`, `Progress`,
`Cancel`, `Done` — echoes the `request_id` the client chose for that `RENDER`. A
`CANCEL` can be in flight past a worker that's already mid-batch, so `FRAME`/`PREVIEW`
payloads for the just-cancelled request may still be in transit when the client
moves on to its next request. The rule that makes "never merge a stale partial into
the next render" mechanical, rather than a race:

> Honor/sum/display a payload iff its `request_id` matches the accumulator's
> **current epoch**; drop everything else.

`Accumulator::begin_request(request_id)` starts a new epoch: it zeroes the buffer,
resets the sample count, and clears any pending preview. Callers **must** call it
before or immediately after sending the next `RenderRequest` — before any reply for
it can possibly arrive — so that bytes for the old epoch still in flight see the
new epoch already in place and get dropped by `apply`. `StreamEvent::Error` is the
one message with no `request_id`; it is never epoch-gated.

`send_cancel` itself does not touch the accumulator — it just writes the `CANCEL`
message. The accumulator naturally stops accepting further payload for the
cancelled request the next time `begin_request` is called for a different id.

## The `BUILD_ID` handshake gate

`handshake::verify_compatible(local: &Hello, remote: &Hello) -> Result<(), Incompatible>`
refuses to pair a viewer and a worker whose `PROTOCOL_VERSION` or `build_hash` disagree.
`build_hash` (`pleochroma::BUILD_ID`) is a hash of `pleochroma`'s crate *version*, a
release-process promise, not of its source. The content hash is the separate
`source_hash` (`pleochroma::SOURCE_HASH`): it travels in `HELLO`/`WELCOME`, but
`verify_compatible` does not compare it (a test pins that a differing `source_hash` with a
matching `build_hash` is compatible), so it is diagnostic only. There is no "close enough" tier for what is compared:
a different protocol version or `build_hash` means refuse, including two peers that both
report an `UNKNOWN_BUILD_HASH` (two unknown builds are never treated as compatible even
with each other).

Two limits of this gate. Both hashes are *self-reported* in the peer's `HELLO`/`WELCOME`,
so it is an accident detector for honest peers on mismatched builds, not a security
control — mutual TLS and the allowlist are the trust boundary. And a server may
deliberately pair a peer that reports an unknown build *library-only* (no radiance ever
flows on that connection) instead of refusing it; `apps/pleochroma-worker`'s
`serve/connection/worker.rs` does this for a client speaking the same protocol version.

Why this can't be relaxed: the whole remote-offload design rests on
`sample_sum += trace_spectral_ray(..)` being valid regardless of which node
computed which term, and that is only true if every node runs the *same* physics.
Two builds that differ by, say, a spectral-MIS weighting fix produce numbers that
both look like plausible radiance — there is no runtime signal (no NaN, no panic,
no obviously-wrong magnitude) that distinguishes "two different physics
implementations summed together" from "a converged render." `pleochroma`'s physics has
already changed many times in quick succession during development, which is
exactly the situation a hand-maintained version number can't catch.

The client re-runs `verify_compatible` itself against the worker's `WELCOME` reply
even though the worker already refused a bad `HELLO` on its own side — two
independent checks, neither trusting the other, for the same reason: there is no
runtime signal that would let either side detect the mismatch on its own if the
other side's check silently failed.

## TLS — a different question from the `BUILD_ID` handshake

`tls::server_config`/`tls::client_config` build a TLS **1.3-only** `rustls`
config (`tls12` is deliberately excluded from the workspace's `rustls` feature
list — this workspace never has a reason to negotiate down). The server config
requires a client certificate (`WebPkiClientVerifier`) — mutual TLS is what
replaces a password in this design.

CA-chain validity only proves "this client certificate was signed by a CA I
trust" — it says nothing about *which* signed client should be trusted.
`tls::Allowlist` closes that gap: a plain-text file of SHA-256 client-certificate
fingerprints (`tls::fingerprint`), one 64-hex-char line per line, `#` comments
allowed. `apps/pleochroma-worker`'s `serve` command checks a connecting client's
fingerprint against it after the TLS handshake completes, and re-reads the file on
every connection (no restart needed to revoke access — delete the line). A
malformed non-comment, non-blank line is a hard load error (fail-closed on
purpose) rather than being silently skipped.

TLS and the handshake module answer two different questions and neither
substitutes for the other: TLS+allowlist answers "may this peer talk to me at
all"; `handshake::verify_compatible` answers "are we running the same ray-tracing
physics." A peer can pass one and fail the other.

## Public API tour

### `scene::SceneState` — a fully-resolved scene, never a name or an id

```rust
pub struct SceneState {
    pub width: u32,
    pub height: u32,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub light_yaw: f32,
    pub light_pitch: f32,
    pub exposure: f32,
    pub max_bounces: u32,
    pub lighting_preset: LightingPreset,
    pub material: GemMaterial,
    pub planes: Vec<GpuFacetPlane>,
    pub girdle_frosted: bool,             // per-facet finishes are re-derived from `planes`
    pub backdrop: f32,                    // tray radiance at exposure 1, 0.0 = the preset's own ground
    pub environment: SceneEnvironment,    // Studio (the fields above) | Hdr { content_hash, width, height }
    pub surface_glare: f32,               // 0.0..=1.0 scale of the first-surface reflection, 1.0 = unscaled; analytic presets only
    pub tools: Vec<ToolPrimitive>,        // concave-facet tool volumes in cutting order; empty for a convex stone
    pub head_shadow_deg: f32,             // head-shadow radius of the lit presets, 0.0..=30.0, 16.0 default
    pub frosted_planes: Vec<u32>,         // ascending indices of the planes a design frosts; empty = none
    pub engine: EngineSpec,              // default Path-Traced, or Ray-Traced options
    pub effects: SceneEffects,            // optional material and surface effects
    pub camera_roll: f32,                 // camera image-plane roll in radians
    pub stone_shadow: bool,               // false by default
    pub tray_reflection: f32,             // 0.0..=1.0 head-on reflectance of the tray, 0.0 = off
    pub hdr_orientation: HdrOrientation,  // yaw in radians + turns-with-camera flag, default = map as decoded
    pub estimator: Estimator,             // Branching (0) | ExitCollection (1), the viewer's choice
    pub camera_pan: [f32; 2],             // last wire field; where the stone sits in the picture, image half-heights (x right, y up), [0, 0] = centred
}
```

Every field from `girdle_frosted` on is `#[serde(default)]` so an older on-disk
`scene.json` still loads; on the wire (postcard) every field is always present. Human-readable output omits
`stone_shadow` when false, `tray_reflection` when 0, `hdr_orientation` when it is the default (yaw 0, not following the camera) and `estimator` when it is `Branching` and `camera_pan` when it is centred, preserving shadow-free, reflection-free, unturned-map and branching JSON. Each worker computes its own
map; no map bytes are transmitted. `tray_reflection` is the "Tray Reflection" setting: the head-on
reflectance `R0` of the surface under the stone, rising towards 1 at glancing angles. A worker
validates it as finite and within `0.0..=1.0`, and builds the mirror from the stone's own geometry.
The same studio-backdrop scope as the Stone Shadow applies.

`hdr_orientation` turns an HDR environment map: the map's own yaw in radians and a flag for whether
it also turns with the camera (the worker lights the scene with the rotation
`HdrOrientation::rotation_for(scene.yaw)`). It is ignored unless `environment` is an HDR map, and a
worker refuses a non-finite yaw. `HdrEnvironment` still names the map asset alone.

`estimator` is the stone estimator of the frame: `Branching` (wire byte 0) or `ExitCollection`
(wire byte 1), one postcard varint just before the pan. The viewer fills it from its own
default and a worker traces with it, never with its own process default, so a frame split between
the viewer and workers runs one estimator. A JSON scene or job
written before the field has no key and reads as `Branching`, which is what it was rendered
with; the variant order is part of the wire format, so new variants are appended only.

`camera_pan` is the "Pan" framing of the camera: where the orbit centre (the stone) sits in the
picture, in image half-heights, `x` to the right and `y` up in the picture's own axes (after the
roll); `[0.0, 0.0]` is the centred camera. The eye moves parallel to the image plane
(`Camera::with_pan`), so the stone is off-centre and seen from a slightly shifted point of view. It
is the eight wire bytes (two `f32`) before the pose, added in protocol 43; a JSON scene or job
written before the field has no key and reads as centred. A worker refuses a non-finite component
or one whose magnitude exceeds `MAX_CAMERA_PAN` (4.0).

`stone_pose` is the stone's pose on the tray in degrees, `[tilt, turn, spin]`
(`pleochroma::render_setup::StonePose`); `[0.0, 0.0, 0.0]` is the stone as designed. It is the last
twelve wire bytes (three `f32`), added in protocol 44; a JSON scene or job leaves the key out at
identity, and one written before the field reads as identity. A worker refuses a non-finite
component and clamps the rest into the pose ranges (tilt 0 to 180, turn and spin 0 to 360).

A viewer stores a material as a name plus a list of custom materials loaded from
its own local SQLite database, and stores a diagram as an id looked up against
`pleochroma-vault`. A remote worker has neither. Sending the *name* or the *id*
instead of the resolved value would compile, serialize, and deserialize just fine
— and then silently render the wrong stone (or the wrong facet geometry) the
moment someone selects a custom material or a diagram the worker's own copy of the
data doesn't happen to agree with, with no error at all. So `SceneState` always
carries the fully-resolved `GemMaterial` and facet-plane geometry, never a name or
an id. It also deliberately excludes local UI/session state (`dirty`, `paused`,
`target_samples`, ...) — none of it changes what a worker computes, since the
viewer already resolves its own accumulation target down to the concrete sample
count it asks for.

`SceneState` round-trips exactly through `postcard` (verified in
`tests/scene_roundtrip.rs` against a material with absorption bands and biaxial
data, every `LightingPreset`, a diamond with empty absorption, and an empty plane
set) — this is exactly the kind of fidelity a name/id-based scheme would have
silently lost.

### Sending a render request

```rust
use pleochroma_net::{client, messages::{RenderRequest, RequestIntent, StreamConfig, TransferMode}};
use std::io::{Read, Write};

fn send_one_request<S: Read + Write>(
    stream: &mut S,
    accumulator: &mut client::Accumulator,
    scene: pleochroma_net::SceneState,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Handshake -- refuses to proceed on a protocol or BUILD_ID mismatch.
    let welcome = client::handshake(stream)?;
    println!("render capability: {:?}, build {:x?}", welcome.render, welcome.build_hash);

    // 2. Send a render request and start a new accumulation epoch for it.
    let request = RenderRequest {
        request_id: 1,
        scene,
        first_sample: 0,
        samples: 256,
        stream: StreamConfig {
            transfer_mode: TransferMode::LiveProgressive,
            cadence_ms: 250,
            preview: None,
        },
        intent: RequestIntent::Interactive, // a live view; exports use `Batch`
    };
    client::send_render_request(stream, &request)?;
    accumulator.begin_request(request.request_id); // before/immediately after sending

    // 3. Drain the reply stream, folding FRAME deltas and PREVIEW snapshots in.
    client::run_client_session(stream, accumulator, |update| {
        println!("{update:?}");
    })?;
    Ok(())
}
```

`client::test_connection` performs step 1 only (handshake, report worker identity/
backend/build compatibility, then the caller disconnects) — this is what a "Test
connection" button in a settings UI should call; no `RenderRequest` is ever sent.

`client::send_cancel(writer, request_id)` writes a `CANCEL` for a request already
in flight; it does not touch the accumulator (the next `begin_request` call is what
actually starts ignoring the cancelled epoch's stragglers). Everything here is
generic over `Read`/`Write` — wrapping an actual `TcpStream` (optionally inside
`rustls::StreamOwned`, built via `tls::client_config`) happens at the call site;
see `apps/pleochroma-worker/src/serve/` for the server side of the same split, and
`apps/pleochroma-cut/src/bridge/remote/remote_render/` for the client side.

### Reading the radiance buffer

```rust
use pleochroma_net::radiance;
use glam::Vec3;

let buffer: Vec<Vec3> = vec![Vec3::new(1.0, 2.0, 3.0); 64 * 64];
let bytes = radiance::encode(&buffer);          // zero-copy POD view via bytemuck
let decoded = radiance::decode(&bytes, 64, 64)?;       // width, height
assert_eq!(buffer, decoded);
# Ok::<(), pleochroma_net::radiance::RadianceError>(())
```

The radiance payload is raw POD `Vec3` bytes (`bytemuck::cast_slice`), never a
serialization framework — this is the hot-path payload (up to ~100 MiB for a 4K
frame), and `postcard` framing overhead has no reason to touch it. On the wire it may
additionally be byte-shuffled and zstd/LZ4-compressed (`radiance::payload`, see
"Payload encodings" above); decoding restores these exact bytes.

## Key invariants (do not "fix" these)

- **Samples are summed, never averaged**, anywhere on the wire.
- **`FRAME` is additive; `PREVIEW` is not.** Never sum two previews, and never sum
  a preview into the full-resolution buffer.
- **A payload's `request_id` must match the accumulator's current epoch** or it is
  dropped, unconditionally — this is the only defense against a stale, in-flight
  reply from a just-cancelled request being merged into the next one.
- **A protocol or `build_hash` mismatch is refused.** The `source_hash` is carried but not
  compared; a peer with no build at all may be paired library-only by the server, which
  then serves it no render capacity.
- **Reads are bounded and incremental.** A control message (anything but a radiance,
  asset or contribution payload) is read with `framing::MAX_CONTROL_FRAME_LEN` (1 MiB),
  and a frame's buffer grows as bytes arrive, so a length prefix alone never commits
  memory. A connection that ends inside a frame is `FramingError::TruncatedFrame`, not a
  clean close (`Io(UnexpectedEof)` at a frame boundary).
- Every message from `RENDER` onward is tagged (`StreamEvent` / `ClientMessage`) —
  do not add a new untagged reply type; that reintroduces exactly the ambiguity the
  tagging exists to remove (see "Tagged envelopes, and why they exist" above).

## Testing

```
cargo test -p pleochroma-net
```

No networking or sockets are exercised — every test runs against a `std::io::Cursor`
or an in-memory `Vec<u8>` (the enrollment claim client's TCP path is exercised from
`apps/pleochroma-worker`'s tests). This includes:

- `tests/scene_roundtrip.rs` — `SceneState` round-trips exactly through `postcard`.
- Extensive inline `#[cfg(test)]` unit tests in every `src/*.rs` module covering
  round-trips, malformed-payload rejection, epoch-gating, and partial/dribbling
  reads.

The beam tile tests (`messages::beam_tiles`) compile only with `--cfg beam_tests`, never under
`cargo test` or `--all-features`. Run them with `bash scripts/run_beam_tests.sh`, or by hand with:

```
RUSTFLAGS="-C target-cpu=x86-64-v3 --cfg beam_tests" CARGO_TARGET_DIR=target/beam-tests cargo test -p pleochroma-net --features render -- messages::beam_tiles
```

An explicit `RUSTFLAGS` replaces `.cargo/config.toml`'s flags, so the line restates its x86-64
baseline (on another architecture leave `-C target-cpu=...` out); the own target dir keeps the
regular build cache.
