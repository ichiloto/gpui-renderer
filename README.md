# Ichiloto GPUI renderer

The protocol 2 retained renderer draws maps, sprites, text and canvas entities
without resending the whole scene for every camera movement. The negotiated
`graphical_canvas` capability draws images at explicit graphical
rectangles, with outlines/underlines and locally positioned text. PHP resolves
image pivots, contain-fit geometry and presentation state. Native resizing fits
the complete canvas uniformly and centers it. Existing field sprite/tile grid
coordinates retain their meaning.

Project-owned image cursors use the existing canvas image path; see
[cursor presentation](#cursor-presentation) for ownership and limits.

The [G1 wire corpus](fixtures/graphical-canvas/manifest.json) and its
[hash manifest](fixtures/graphical-canvas/SHA256SUMS) remain historical
stateless validation references. In the current contract, identified canvas
elements persist until replaced or removed through retained operations. Canvas
text uses transparent null backgrounds, while explicit colors paint opaque cells.
Canvas and visible world content remain separate presentations.

The additional v2 `canvas_clip_opacity` capability requires `graphical_canvas` in
the enabled capability set advertised in ready. It adds optional `clipRect` to canvas
images and text layers, and optional `opacity` to canvas text layers. See the
[independent clipping/opacity wire cases](fixtures/canvas-clip-opacity/manifest.json)
and their [hash manifest](fixtures/canvas-clip-opacity/SHA256SUMS).

`clipRect` is `{ "x": 40, "y": 40, "width": 80, "height": 24 }` in absolute
canvas logical coordinates. Origins must be finite and nonnegative; dimensions
must be finite and strictly positive, with the whole rectangle inside the canvas.
It intersects the existing image destination or text grid without changing its
position, dimensions or image source mapping. A disjoint or edge-touching clip
paints nothing. Original destination, text, asset and source validation still
applies even when hidden. For gauges, keep the image destination at full track
width and vary only the clipping width; omit a zero-fill image.

Text opacity accepts finite values from 0 through 1 and applies to glyphs and
explicit cell backgrounds. Omission means full opacity. Presence of either new
field, including explicit text opacity `1`, requires negotiation; explicit null
and unknown fields are rejected. Existing image opacity requires only
`graphical_canvas`. Replacing an element without those fields removes its previous
clip/fade. Source crops, caches, budgets and screen text behavior are unchanged;
PHP continues to resolve motion and fade timing.

This extension has headless macOS validation. Text clipping/fading also passed
the focused native glyph scenario below, followed by an isolated ordinary Game
battle playtest on macOS. Linux/WSLg/Windows validation remains pending. See
[availability and installation](#availability-and-installation) for current delivery status.

The separately negotiated v2 `canvas_glyph_effects` capability also requires
`graphical_canvas`. A canvas text layer may then include:

```json
"glyphEffects":{"outline":{"width":2,"color":{"kind":"rgb","r":8,"g":15,"b":29}},"shadow":{"offsetX":0,"offsetY":2,"sigma":1,"opacity":0.7,"color":{"kind":"rgb","r":8,"g":15,"b":29}}}
```

Both nested objects and every shown field are required when `glyphEffects` is
present. Values must be finite: outline width 0..4, shadow offsets -8..8, Gaussian
sigma 0..4, shadow opacity 0..1. Null and unknown fields are errors. Existing
`clipRect` and text opacity still independently require `canvas_clip_opacity`.
The [90-case effects corpus](fixtures/canvas-glyph-effects/manifest.json) and its
[hash manifest](fixtures/canvas-glyph-effects/SHA256SUMS) freeze this extension.

The original grid, cell pitch, scalar positions, run order and colors remain
authoritative. The runtime uses system monospace with measured sizing; foreground
and real contour strokes share the same COSMIC Text/Swash glyphs. Effects expand
only paint bounds: with `R = outline.width + ceil(3 * shadow.sigma)`, each side's
padding is `ceil(R + max(0, signed shadow offset toward that side))`. The example
reserves left/top/right/bottom 5/5/5/7 logical pixels. The entire expanded rectangle
must fit the canvas even with zero opacity or a tiny clip. `clipRect` intersects
that expanded footprint. Engine owns placement, motion, timing and whole-block
removal. No font assets, semantic label parsing or native animation clock are used.

This glyph path uses exact direct dependencies COSMIC Text **0.14.2** and Swash
**0.2.10**, retaining GPUI **0.2.2**. Text without `glyphEffects` keeps the existing
GPUI text path. Font appearance can vary between hosts. Effects require scalable
glyph contours; unavailable system glyphs reject the complete candidate before
visible replacement instead of substituting an approximation. See
[glyph implementation and validation](docs/glyph-effects-validation.md).

Historical stateless native fixture results remain in the linked validation
documents. For this retained build, use the Engine's fixture-only
`tools/gpui-frame-smoke.php` with a local `--renderer` path and an absolute
`--asset-root`; it has no Game audio path. Native validation of this change
remains pending while the host is locked.

## Local image compositing

The optional v2 capability `canvas_compositing` requires `graphical_canvas`.
It adds retained `canvas_composite` entities; adding one requires negotiation,
and `remove` clears it. The local development installation
described below includes this capability; it is not a published renderer release.
See the [sample frame](fixtures/compositing/frame.json), using the existing
`fixtures/test-sprite.png` and `fixtures` as its asset root.

Each composite has `{id,width,height,destination,layer,operations}` and optional
`opacity` (default 1) and `clipRect`. Dimensions are integer local raster units;
destination and clip use canvas coordinates. Operations paint into an initially
transparent image in array order. The final image uses ordinary source-over.
Screen blending never reads arbitrary canvas layers: put the intended backdrop
inside the composite first. Equal-layer canvas order is images, composites,
indicators, text, with stable array order within each type. PHP owns all motion,
phase, positions, theme policy and lifetime; packets contain no timers or recipes.

Operations are strictly tagged with `type`:

| Type | Required fields | Optional fields |
| --- | --- | --- |
| `image` | `asset`, `destination` | `source`, `displacement`, `opacity`, `blend`, `masks` |
| `fill` | `destination`, `brush` | `opacity`, `blend`, `masks` |
| `stroke` | `points`, `width`, `brush` | `opacity`, `blend`, `masks` |

Rectangles use `{x,y,width,height}`. Image `source` is normalized to the current
decoded PNG (default the whole image), unlike existing integer `sourceRect`.
Sampling is bilinear in premultiplied sRGB channels, clamped to the selected
source extent. A displacement is `{columns,rows,offsets,masks?}`: row-major
`[dx,dy]` points interpolate over the destination. Offsets are local destination
units added to inverse sampling; positive x samples farther right. Its masks
multiply displacement strength, while operation masks multiply output alpha.
This permits a compact uniform grid or narrow strip grid without image-per-tile
packets. Stroke points are an open polyline with round caps/joins; producers
flatten authored curves into bounded points. Paths may cross the target edge.

Brushes are `solid` with `color`, `linear` with `[x,y]` `start`/`end` and `stops`,
or `radial` with `center`, `[rx,ry]` `radius` and `stops`. Stops have `offset`,
structured `color` (the existing RGB/ANSI object), and optional `opacity` (1).
Offsets increase strictly from 0 to 1; colors/alpha interpolate premultiplied.
Radial position is elliptical radius, 0 at center and 1 on the outer ellipse.
Operation `blend` is `source_over` (default) or `screen`; opacity defaults to 1.

Masks are intersected by multiplying coverage. `polygon` has `contours`, a union
of closed point lists (each uses even-odd interior); `ellipse` has `center` and
`radius`; both accept `feather` (0) and `invert` (false). Polygon coverage uses
the nearest edge with interior sign; ellipse feather uses signed distance along
the radial ray (exact distance for circles). Inversion reverses that sign before
smoothstep feathering. Zero feather uses a one-local-pixel boundary coverage
ramp. `image_alpha` has `asset`, `destination`, optional normalized `source` and
`invert`; it samples current image alpha, so replacement artwork needs no frozen
hash or duplicated dimension metadata. Mask images use the same asset validation.

All values must be finite; unknown keys/types and explicit null are rejected.
Local image/fill destinations fit their target; canvas destinations/clips retain
the existing full-rectangle validation. Limits per frame: 8 composites, 256
operations, 8,388,608 target pixels, 16,384 displacement nodes. Each target axis
is 1..4096 and area at most 4,194,304. Each grid axis is 2..64, offsets within
±4096. Each mask list has at most 8 entries; a polygon has 1..8 contours of
3..128 points without duplicate consecutive vertices. Strokes have 2..256 points
and width `(0,256]`. Points are within ±16384, radii `(0,16384]`, feather 0..4096,
gradients 2..8 stops, opacity 0..1. IDs use existing 256-byte rules.

The compositor runs on the protocol reader thread before frame acceptance.
Sources share the existing per-frame 1024-image/64 MiB decode budget. Its own
64 MiB pool accounts live output generations (including queued/visible frames),
new outputs, two temporary coverage buffers, an 8 MiB mask cache and up to
16 MiB of reusable first-image pixels. Ordinary region and glyph/tile cache
budgets are unchanged. Only current composite outputs are retained for reuse;
older generations are weakly tracked until their owners release them. Geometry
masks and immutable first images reuse bounded LRU entries. Source identities
participate in keys; changed artwork invalidates derived pixels.
These are CPU image/scratch bounds, not a total process or GPU-memory guarantee.
Composite raster density is one pixel per declared local unit; normal canvas
scaling then applies, without rebuilding pixels during a window resize.

Conservative work is limited to 268,435,456 units per candidate. Each new output
costs `2*width*height`, plus each clipped operation bounding-box area multiplied
by image 16 (+8 for displacement), fill 4, or stroke `4+pointCount`. Every mask
list use adds the same box area times `2+sum(maskCost)`: polygon vertex count,
ellipse 8, image-alpha 16. Masks are charged cold even when cached, preventing
cache eviction order from bypassing admission. Reused whole outputs cost zero.
Budget failure rejects the candidate rather than skipping effects.

CPU checks and the bounded synthetic workload run without any window or audio:

```sh
cargo test --release --locked composite_tests
cargo test --release --locked benchmark_representative_day_night_composition -- --ignored --nocapture
```

These checks are not GPU timing, native visual acceptance or Windows/Linux/WSLg
validation. The real-packet results below do not meet the 16.7 ms target.

On the macOS arm64 development host, the release-mode synthetic test (25 changing
snapshots, first cold, four retained generations) measured:

| Workload | Cold ms | Warm mean ms | Warm p95 ms |
| --- | ---: | ---: | ---: |
| Day | 196.02 | 13.46 | 13.61 |
| Night | 203.98 | 13.04 | 13.74 |
| Both themes | 401.55 | 26.63 | 28.28 |

Outputs are 1350×720 with synthetic 1717×916 source paintings, full-surface sky
mask boxes, three water ribbons and 105 moving screen strokes per theme; Night
also includes masked flame and radial glow. The logo is excluded. Peak reserved
compositor bytes were 57,721,061. These numbers exclude PNG decode, protocol I/O
and GPU/native drawing. Warm single-theme work fits 16.7 ms here; the overlap
fits 33.3 ms but not 16.7 ms. Before first-image reuse, respective warm means
were 51.91, 85.47 and 137.93 ms. The renderer does not reduce packet cadence.

An optional real-packet CPU replay test accepts NDJSON starting with a hello
(including the actual asset root), then complete frames. A line may instead be
`{label,message}`. With `ICHILOTO_COMPOSITE_REPLAY` pointing to that file and
`ICHILOTO_COMPOSITE_REPORT` to an output directory, run
`cargo test --release --locked replay_engine_composite_packets_without_a_native_window
-- --ignored --nocapture`. It writes preparation timings/resource reports and
individual CPU composite PNGs at the first and every 30th subsequent frame
(override with positive `ICHILOTO_COMPOSITE_CAPTURE_EVERY`); these are not native
window screenshots. It reads
current assets without launching PHP, a game, audio or a GPUI window.

The 2026-09-22 Engine/Game title export was replayed on the same macOS arm64
host with 60 frames per scenario and four retained generations. These timings
include frame validation, asset/cache preparation and composition, but exclude
wire parsing, PHP production, GPU upload and native drawing. The first frame
is listed separately; subsequent frames include any newly encountered resources.

| Actual title scenario | First ms | Subsequent mean ms | Subsequent p95 ms | Subsequent max ms |
| --- | ---: | ---: | ---: | ---: |
| Day with logo | 223.42 | 21.46 | 23.98 | 24.95 |
| Night with logo | 227.09 | 20.82 | 24.64 | 28.19 |
| Day → Night transition | 213.52 | 15.50 | 19.42 | 138.06 |
| Reduced motion | 44.02 | 3.28 | 6.33 | 21.38 |

All 240 frames passed validation/preparation. Every subsequent Day/Night frame
fit 33.3 ms, but none fit 16.7 ms. The transition had two additional stalls:
40.26 ms for the newly encountered Night painting and 138.06 ms for its first
composite prefix/mask preparation. Its middle phase fades ordinary paintings
while ambient effects are off; it does not render both animated composites
simultaneously. Peak compositor reservation was 42,340,486 bytes; the independent
decoded-source pool peaked at 34,871,296 bytes. This establishes protocol and
CPU-output compatibility, not smooth 60 fps or native visual acceptance. No
packet cadence reduction or platform-specific fallback is applied.

## Window activation

Request `window_activation` explicitly in v2 hello.requiredCapabilities to subscribe.
The renderer then sends
`{"protocol":2,"type":"window_activation","active":true}` after `ready`,
then only when OS activation changes. Unnegotiated sessions receive no such
events. This event subscription is not implicitly enabled by drawing support.
Title and Credits do not require it for graphical presentation: without it, native
focus-based pausing is unavailable while scene/modal pausing remains supported.
The GPUI observer is owned by the window entity and stops at teardown.
PHP decides whether to pause elapsed time, defer local-time changes or clear input.
`active` means OS focus, not visibility: visible unfocused windows are inactive.
Hidden/minimized/occluded status is not reported or inferred from paint cadence.
No native platform lifecycle acceptance is implied by CPU protocol tests.

A standalone native presentation and keyboard surface using pinned **GPUI 0.2.2**.
Its production input accepts retained protocol 2 sessions. PHP owns actions, input bindings, movement,
collision, scenes, battle state, camera conversion, timing and saves. Rust receives
bounded presentation changes and forwards key identities. It has no game loop,
audio, ANSI parser, terminal emulator or gameplay meaning for layer IDs/numbers.

## Availability and installation

This section is the current installation status; dated validation documents and
receipts describe their original checkpoints, including superseded binaries.
On 22 September 2026, the optimized development executable containing local
image compositing, window activation and shared canvas destination sampling,
SHA256 `781046dfb2c57b0b472e75a0d625bd65606ff7dfaf30224d8914abc08d20d6a8`,
was packaged and installed in the normal Engine package at
`resources/renderers/installed/gpui/darwin-arm64/Ichiloto Renderer.app/Contents/MacOS/gpui-renderer`.
The existing packager reused the release executable, and Engine's installer
performed its dry run and installation. The release unit suite passes 121 tests
with three opt-in checks ignored; formatting, release Clippy and the release
build also pass on macOS arm64. Earlier real-packet CPU replay results are
recorded above. Native visual acceptance of the latest canvas sampling and
activation behavior remains pending. The subsequent five-battle capture check
stopped before launch when screen-capture preflight reported access unavailable;
it produced no native screenshots. Linux, Windows and WSLg were not tested.

That local installation is not a published renderer release or a Console
installer. Pulling the Engine repository does not install GPUI: a clean
`resources/renderers/` directory contains a README, while generated installed
packages and manifests are ignored by Git. Public player delivery remains
unfinished; it must supply compatible, verified platform packages without
requiring Rust, a compiler or a source checkout.

WSL/WSLg runs the Linux renderer with Linux PHP. A native Windows executable is a
different target, and Engine's native Windows process transport remains a separate
unsupported boundary. Removing this renderer's platform startup rejection does not
provide those packages or establish end-to-end platform support.

## Cursor presentation

Engine owns actor/target identity, anchoring, selection and oscillation timing;
Game owns the cursor images. The intended presentation uses an above-head actor
cursor and high-contrast animated target cursors pointing down or inward from a
side. Directional images with authored outlines/glow use existing `CanvasImage`
placement and opacity. They need no rotation primitive, native animation clock
or new protocol capability, and avoid font-dependent silhouettes and extra text
layers. The existing 64-text-layer limit remains unchanged. Renderer draws the
submitted assets without inferring gameplay meaning from IDs or marker shapes.

## Developer build and validation

Run these commands from the root of this separate `ichiloto/gpui-renderer`
repository, alongside [Cargo.toml](Cargo.toml). Engine's `resources/renderers/`
directory is an installation destination and contains no Rust project. These are
development and packaging instructions, not player setup steps. Reuse the
accepted installed executable when source changes do not require a new binary.
Use repository branches and the normal Game checkout for fixes and playtesting;
do not accumulate standalone runtime copies. Before a game playtest, verify music
and sound effects are muted and preserve existing mute choices. The synthetic
Renderer fixtures below contain no audio.

Repository utilities use PHP 8.2 or later with JSON and zlib; no Python runtime
or Composer installation is needed. Run the windowless utility regressions with
`php tests/scripts/run.php`. They compare retained analysis results and exercise
process drivers through PHP test doubles, not native rendering. The Darwin-only
historical `sample-window.php` adapter also requires PHP FFI for its original
`CLOCK_UPTIME_RAW` markers and `/usr/bin/sample`; other analysis and fixture tools
do not require FFI. Evidence helpers share code under `scripts/lib/`, so run them
from a complete checkout. Their `--base`, `--pid` and `--out` options allow replay
into a new directory without overwriting retained measurements.

Rust **1.98.1** is pinned in `rust-toolchain.toml`; commit and use `Cargo.lock`.

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
php ../engine/tools/gpui-frame-smoke.php --renderer="$PWD/target/release/gpui-renderer" --asset-root="$PWD/fixtures" --duration=15 --change-after=7
```

The Engine fixture smoke is silent and opens one bounded window. The older
`scripts/native-tile-smoke.php` and its [installed-renderer check](docs/s8-b-installation.md)
describe the prior stateless endpoint and are historical evidence only.
For a retained wire capture, `scripts/native-retained-replay.php --binary
target/release/gpui-renderer --replay /absolute/capture.ndjson --evidence-dir
/absolute/output --hold-frame 1 --observe-seconds 15` opens one silent fixture
window, checks every generation acknowledgement, and closes it automatically.
The tool runs only the renderer; it does not start the Game or its audio system.

Ordinary gameplay uses Engine's installed optimized executable. When a new build
is necessary, `target/release/gpui-renderer` is the packaging and performance
validation input. `cargo build --locked` produces an unoptimized
`target/debug/gpui-renderer` for development/debugging; it is not the performance
baseline. A matched Last Legend investigation found long foreground paint work in
the debug build and substantially shorter frame handoff/draw intervals with the
same source compiled in release mode. This is a build-profile comparison, not a
scheduling or gameplay change; see [the measured investigation](docs/burst-investigation.md).

## Packaging and installation

`scripts/package.php` builds the optimized executable and produces a verified
renderer package under `dist/`: a staged directory and a `.tar.gz`, each
carrying `renderer-package.json` with the renderer id, platform id, package
version and a SHA-256 for every payload file. On macOS the payload is the
`.app` bundle with its `Info.plist` (from `resources/macos/Info.plist`)
preserved for native application identity; on Linux and Windows it is the
bare executable. Cross-compiled targets pass `--platform` together with
`--binary` pointing at that target's built executable.

```sh
php scripts/package.php
```

For Console's explicitly declared source-development checkout, the builder also
provides a read-only description:

```sh
php scripts/package.php --describe --out=/private/output/directory
```

Stdout is exactly one JSON object with `renderer` (`gpui`), `platform` (host or
`--platform`), `profile` (`release`), `fingerprint` (SHA-256), and
`packageDirectory` (absolute `--out/gpui-<platform>-<version>`). The directory may
not exist yet. Description runs no compiler or subprocess and creates no files;
errors go to stderr with a nonzero exit code. Running without `--describe` and
with the same `--out` builds and stages that package, retaining human-readable
build output. Console can compare descriptions before and after preparation to
reject concurrent input changes. Normal builds discover the executable from
Cargo's compiler-artifact output, including configured target directories.
At build time only, the builder queries `rustc -vV` (or `RUSTC`), verifies that
its host matches the package host, and explicitly selects that native target.
This overrides a foreign Cargo target configuration rather than mislabelling a
cross-compiled executable. Explicit cross-platform packaging still uses
`--platform` and `--binary`; `--skip-build` retains the conventional
`target/release` default.

The fingerprint covers Cargo manifests/lockfile, Rust source trees, optional
`build.rs` and toolchain declarations, bundle resources, the packaging script and
its helpers, applicable ancestor/Cargo-home configuration, and relevant declared
compiler/linker/build environment. It excludes documentation, evidence, build
outputs, Git metadata and game artwork. It is a source/build-context cache key,
not verified compiler provenance: description does not query compiler versions,
hash installed toolchains, resolve external build-script inputs or certify OS
libraries. Replacing those external inputs without changing declarations may
require explicit preparation again. `--describe` rejects `--binary` and
`--skip-build`; manual packaging cannot certify an arbitrary executable as a
build of the described source. Source preparation belongs to Console's declared
development path; Engine's installed-renderer lookup remains build-free.

Pure PHP regression checks (no Cargo or native windows):

```sh
php tests/scripts/run.php
php tests/scripts/package.php
```

Installation is owned by the Console, which verifies every hash before
staging anything into the Engine's `resources/renderers/installed/` boundary
and backs up any existing installation:

```sh
ichiloto renderer:install dist/gpui-<platform>-<version>.tar.gz
```

That installs into the current project's Engine package. Development staging
into an Engine checkout uses `--engine <path>`. The package format is
renderer-agnostic; future renderer implementations publish the same artifact
and install through the same command.

Engine's GPUI launch now retains the shared presentation buffer while skipping
physical terminal drawing. The [Garden of Roads comparison](docs/garden-performance.md)
records that improvement, the shared style-processing fix, and validation limits.

The earlier `scripts/native-smoke.php` exercises stateless protocol 1 and 2
fixtures and is historical evidence; it cannot validate this retained build.
Use the Engine's silent retained fixture smoke or `scripts/native-retained-replay.php`
on a graphical desktop. The replay checks acknowledgements and bounded process
lifecycle, but does not prove keyboard input or visible GPU pixels. Its
`--evidence-dir` records the input, output, and receipt.

Native startup uses one window policy across GPUI backends: request desktop-managed
maximization, with a centered restore size fitted to GPUI's suggested window bounds.
The desktop determines the actual content area, including panels, decorations and
display scaling. The shared viewport transform fits and centers the complete grid
or graphical canvas on every resize. The window remains movable, restorable and
resizable. Display bounds are not treated as measurements of usable work area.
See the [portable window correction and validation](docs/portable-window-validation.md).

Validated on macOS/Apple Silicon, with Xcode/SDK/Metal toolchain installed.
Linux/WSLg and native Windows compilation and desktop execution still require
validation on those platforms; the renderer has no operating-system startup
rejection. Native renderer backend support alone does not establish support for
Engine's process transport or availability of an installed platform package. Upstream
`block 0.1.6` and `proc-macro-error2 2.0.1` report future-compatibility warnings.
The earlier glyph-effects source passed 106 optimized tests, Clippy and its
recorded native checks; see [glyph-effects validation](docs/glyph-effects-validation.md), the
earlier [S7-R validation](docs/s7-r-validation.md) and [S1 validation](docs/s1-validation.md).

## Session and channel contract

The production renderer accepts **protocol 2 retained sessions**. One UTF-8 JSON
object travels on each NDJSON line: stdin is input, stdout is protocol output,
and stderr is diagnostics. The previous protocol 1 and stateless protocol 2
frame decoders remain available only to historical tests. They are removed
from the production input path.

A session begins with one hello:

```json
{"protocol":2,"type":"hello","title":"Last Legend","assetRoot":"/absolute/path/to/assets","grid":{"columns":135,"rows":36,"cellWidth":10,"cellHeight":20}}
```

`assetRoot` must resolve to an existing directory. The grid fixes cell geometry
for the session, independently of the native window size. Hello opens one
resizable window and emits `ready`. A second hello or a mixed-version message is
an error. `requiredCapabilities` is a mandatory minimum; `ready.capabilities`
reports the available drawing features. `window_activation` is an explicit
event subscription. Before hello succeeds, an error may use the protocol 1
envelope; this is not a downgrade.

### Retained frames

A frame changes identified presentation entities rather than replacing every
cell. For example:

```json
{"protocol":2,"type":"frame","frame":1,"baseGeneration":0,"generation":1,"reset":true,"present":false,"operations":[{"op":"put","kind":"world","id":"map","value":{"columns":2,"rows":1,"cellSize":48,"layers":[{"id":"map:terrain","layer":-99,"kind":"gameplay"}]}},{"op":"worldRows","id":"map","rows":[{"row":0,"cells":[{"glyph":".","foreground":null,"background":null,"displayWidth":1,"ownerLayerId":"map:terrain"},{"glyph":" ","foreground":null,"background":null,"displayWidth":1,"ownerLayerId":"map:terrain"}]}]}]}
{"protocol":2,"type":"frame","frame":1,"baseGeneration":1,"generation":2,"present":true,"operations":[],"viewport":{"scale":1,"origin":{"x":0,"y":0},"clipRect":{"x":0,"y":0,"width":1350,"height":720},"worldId":"map","worldOrigin":{"column":0,"row":0},"textLayerIds":[],"spriteIds":[]}}
```

`generation` is a positive, strictly increasing delivery number;
`baseGeneration` must equal the last accepted generation, except on `reset`.
`frame` labels the presentation transaction and is independent of generation.
`reset:true` starts a fresh retained scene and can recover after a rejected or
lost update. Each line is limited to 4 MiB and 4096 operations. A
`present:false` chunk stages changes without changing the visible scene; its
viewport must be omitted. The final `present:true` chunk validates and commits
the complete scene atomically. An empty operations list with a changed viewport
is a camera-only frame; unchanged scenes are not decoded again.

Accepted chunks emit
`{"protocol":2,"type":"frame_ack","generation":2,"frame":1,"presented":true}`.
The same event uses `presented:false` for a staged chunk. A rejected update
keeps the last visible scene and emits `frame_rejected` with `generation`,
`expectedGeneration`, `message`, and `resyncRequired:true`. The sender must
reset rather than retry a dependent delta. Native content-size or device-scale
changes emit `{"protocol":2,"type":"resized"}`; the Engine coalesces this signal
and sends an updated presentation. `shutdown` closes the session.

Operations use one of these shapes:

| Operation | Effect |
| --- | --- |
| `put` with `kind`, `id`, `value` | Insert or replace a `world`, `text`, `sprite`, `canvas`, `canvas_image`, `canvas_indicator`, `canvas_text`, or `canvas_composite` entity. |
| `remove` with `kind`, `id` | Remove that entity. |
| `worldRows` with `id`, `rows` | Replace complete, indexed owner-glyph rows of a world. |
| `worldTiles` with `id`, `layerId`, `rows` | Replace indexed, sparse tile-candidate rows of one world layer. |
| `textRows` with `id`, `rows` | Replace indexed runs of one screen text layer; an empty row clears it. |

A world definition supplies bounded logical `columns`, `rows`, its square
`cellSize` in logical pixels (1 to 256; Ichiloto uses 48, RPG Maker's tile
size), and ordered layers. The field is drawn at that pitch, independent of the
session text grid: world cells, field glyph text (with a font fitted to the
square cell), and the text layers and sprites named by the viewport all use it.
Only unlisted screen text and sprites keep the text grid's cell pitch. Each layer has an `id`, numeric `layer`, `kind` (`gameplay` or
`decoration`), and optionally an asset-root-relative atlas and source-rectangle
catalog. Every world row must be supplied before presentation; a row may be
shorter than `columns`, leaving an unpainted trailing background. Owner cells
carry one glyph, nullable structured foreground/background, display width, and
the owning gameplay layer ID. Tile candidates refer to source indexes at world
coordinates. The renderer projects only visible rows and columns; camera
movement does not resend the map. World limits are 16,384 on each axis and
1,048,576 logical cells and tile candidates, with at most 64 layers. Retained
source data has a 64 MiB budget; staged plus visible source data is bounded at
128 MiB. These are source-state estimates, separate from decoded images; the
sum of live prepared images referenced by a retained scene is limited to 256 MiB.
Existing 64 MiB limits still apply independently to decoded PNG sources,
prepared regions, and composite output work.
The world source estimate is `sum(64 + UTF-8 glyph bytes + UTF-8 owner-layer ID
bytes)` for supplied owner cells, plus `16` per retained tile candidate and
`8192` per world layer. The logical cell ceiling does not guarantee that a
fully populated world fits the separate source budget; producers should
preflight both limits before uploading it.

A missing, corrupt, inaccessible, or out-of-range world atlas is diagnosed and that
layer paints its retained glyph fallback. It does not reject an otherwise valid
scene or hide an asset problem. Asset resolution never reads outside the asset
root. Malformed entity values and exceeded budgets reject the update.

Screen `text` values contain `id`, numeric `layer`, stable `order`, and
`runs`; each run has `row`, `column`, `text`, and required nullable
`foreground`/`background`. Every covered cell is opaque, including a space.
Absent cells are transparent. A `sprite` value uses the shared sprite fields
plus `order`. Canvas uses a `canvas` root with `id:"canvas"`, width and height,
and the existing image, indicator, text, and composite DTOs as individually
identified operations. Canvas and a world viewport cannot be visible together.

### Camera and selected content

`viewport` omission keeps the last transform; explicit `null` clears it. A
viewport has a finite positive scale through 8, a logical-pixel origin and clip
rectangle inside the session surface. `worldId` and signed
`worldOrigin:{column,row}` select the retained map. `worldId` may be omitted
for screen-only zoom, in which case the world origin must be zero. Only the
world subtracts `worldOrigin`. Named `textLayerIds` and `spriteIds` are
already screen-projected; they receive the scale, origin, and clip but no world
subtraction. Unlisted text and sprites keep their ordinary screen position.
The complete logical surface is then fitted and centered in the resizable native
window. Wide world glyphs use their full display width; a later opaque screen
cell covering either half suppresses the whole underlay glyph.

### Structured colour

```json
{"kind":"ansi16","index":9}
{"kind":"ansi256","index":208}
{"kind":"rgb","r":255,"g":135,"b":175}
```

`ansi16.index` accepts integers 0..15; `ansi256.index` and each RGB component accept
integers 0..255. Unknown kinds/fields, missing fields, negative/fractional values,
strings and out-of-range numbers are errors. No alpha channel or CSS/ANSI strings.
Default foreground is **#D9E1E8**, default background **#111820** (both opaque).

ANSI16 uses this renderer-owned dark-terminal palette. ANSI256 indices 0..15
reuse it. Indices 1..15 exceed 4.5:1 contrast against the default background;
index 0 stays literal black. Bright variants remain lighter than normal variants.

| Index | RGB | Index | RGB |
| --- | --- | --- | --- |
| 0 | #000000 | 8 | #7F8C9A |
| 1 | #E06C75 | 9 | #FF8C95 |
| 2 | #98C379 | 10 | #B3E38F |
| 3 | #E5C07B | 11 | #FFDFA3 |
| 4 | #61AFEF | 12 | #8CCAFF |
| 5 | #C678DD | 13 | #E5A3F5 |
| 6 | #56B6C2 | 14 | #83DCE5 |
| 7 | #C5CED8 | 15 | #FFFFFF |

ANSI256 indices 16..231 use the standard xterm 6×6×6 cube with component levels
`[0,95,135,175,215,255]`: for `n=index-16`, red index `n/36`, green `(n/6)%6`,
blue `n%6` (integer division). Indices 232..255 are grayscale `8+10*(index-232)`.
Explicit RGB remains literal. An explicit background uses the same colour mapping
as foreground, including opaque spaces; colours are never changed in response to
adjacent pixels. The palette target concerns the default background, not every
possible authored foreground/background combination. PNG pixels are not tinted.

## Sprite geometry and safety

The shared sprite DTO uses nonempty unique `id`, relative PNG
`asset`, signed 32-bit cell coordinates `x/y`, positive logical pixel `width/height`,
`anchor:"bottom_center"` and signed 32-bit `layer`. Off-grid sprites are valid and
clipped to the presentation surface. Tint, effects and animation state are not
part of this renderer.

With `sprite_source_rect` negotiated, an optional `sourceRect` selects image pixels:

```json
{"id":"player","asset":"South.png","x":8,"y":4,"width":32,"height":48,"anchor":"bottom_center","layer":100,"sourceRect":{"x":256,"y":0,"width":256,"height":256}}
```

Source `x/y` are nonnegative integers; source `width/height` are positive integers.
All four fields must fit unsigned 32-bit integers, their sums must not overflow,
and the rectangle must fit the actual decoded PNG. Fractions, floating-point
notation, non-finite values, strings, missing members, unknown members and explicit
`sourceRect:null` are rejected. Crops without negotiated capability are rejected.
Any invalid crop rejects the complete frame and preserves the last snapshot.

Omitting `sourceRect` retains the existing full-image drawing behavior, including
GPUI's contain fit. A supplied rectangle fills the destination `width/height`;
these dimensions, cell coordinates, bottom-center anchor and layer are independent
of sheet dimensions. The full sheet is scaled/translated behind a destination-sized
GPU clip mask. Actor sheets use no CPU cropping or generated frame images.
PHP chooses each rectangle and owns animation timing; Rust has no animation clock.
See [S8-A renderer validation](docs/s8-a-validation.md).

```text
feetX = (x + 0.5) * cellWidth
feetY = (y + 1.0) * cellHeight
left  = feetX - width / 2
top   = feetY - height
```

`cellWidth` and `cellHeight` are the world's square `cellSize` for sprites
named by a world viewport, and the session grid's cell otherwise. A one-cell
field character sent at `width = height = cellSize` fills exactly its cell.

Geometry uses logical presentation pixels (1× corresponds to macOS points).
The viewport transform below places the grid inside native content, below the
titlebar. Larger sprites never change cell pitch; font advance never controls cell
positions. Retina backing scale applies equally to text and images.

All rendered text, including dialogue, menus and HUDs over graphical maps, uses
the same metrics-based sizing. Public GPUI font metrics measure the selected
Menlo/monospace font's character advance and ascent + descent per em. Font size
fits within 95% of cell width/height, with an em cap at cell height; the existing
cell pitch and full-cell line height stay fixed. The logical size is cached once
per session and multiplied by the shared viewport scale. Missing/invalid metrics
retain the previous bounded fallback. Optional tracing emits `text_metrics` with
the measured advance, line extent and chosen size. See [readability validation](docs/readability-validation.md).

## Logical grid and native viewport

Engine's graphical default uses the complete battle layout: **135 columns × 36
rows**, independently of the launching terminal. At the registered 10 × 20 cell
size this gives a 1350 × 720 logical surface, shared by fields, menus and battles.
Large maps scroll inside that area; opening a menu or battle does not resize it.
Explicit developer dimensions still override the default. Engine selects these
dimensions before the session; GPUI does not infer them from visible content.
See [battle viewport validation](docs/battle-viewport-validation.md).

The hello fixes the legacy logical grid for the entire session:
`logicalWidth = columns * cellWidth`, `logicalHeight = rows * cellHeight`.
An active negotiated graphical canvas supplies its own logical width and height;
omitting it returns to the unchanged legacy grid. Native window dimensions are
independent. Every paint uses the current logical surface, the actual
`Window::viewport_size()` and the pure `ViewportTransform` model:

```text
scale = min(1, viewportWidth / logicalWidth, viewportHeight / logicalHeight)
presentedWidth  = logicalWidth * scale
presentedHeight = logicalHeight * scale
offsetX = (viewportWidth - presentedWidth) / 2
offsetY = (viewportHeight - presentedHeight) / 2
```

Smaller windows scale the complete surface uniformly. Larger windows keep 1×
rendering centered with letterboxing. Text pitch, font size, line height, opaque
cell backgrounds, sprite origins/dimensions, bottom-center anchors, canvas images
and indicators share this transform. The inner surface clips off-grid sprites at
the logical grid boundary; the outer viewport paints the default background.
There are no scrollbars or
independent X/Y scaling. Resizing requests a GPUI repaint; it never modifies stored
frames, negotiates a grid, sends protocol resize events or changes gameplay/camera
coordinates. A zero-sized viewport paints no surface and retains the focus root.
Large maps can extend beyond this fixed logical viewport: PHP's camera scrolls the
visible portion and sends new snapshots. A negotiated selected frame viewport
can scale just that visible field area while the session grid, menus and HUD stay
fixed. The renderer does not fit an entire map unless its caller deliberately
sends the entire map as the logical surface.

`window_layout.rs` uses the same public GPUI path on every backend. It selects the
primary display (first display fallback), fits centered restore-size hints inside
that display's `default_bounds()`, and requests `WindowBounds::Maximized`. GPUI
delegates maximization to the desktop, which supplies the actual content area.
The player can restore, move and resize the window using normal native controls.

Pinned GPUI's `PlatformDisplay` exposes no usable-area API. Its suggested bounds
are used only for restoration hints; no taskbar, Dock, menu or decoration sizes
are guessed. Native placement remains the desktop's decision, and the game is
centered within the resulting content viewport. Tests cover positive and negative
origins and small, portrait and fractional bounds. Native validation covers the
macOS development display, not a physical multi-monitor setup or Linux/Windows
desktop. See the [portable correction](docs/portable-window-validation.md) for
backend evidence and validation limits. Dated earlier viewport receipts describe
the former AppKit-specific implementation.

The logical size, initial native-size policy, actual viewport and paint transform
are separate boundaries. Future configuration should give developers and players
options for fixed-size, resizable and fullscreen presentation, preferred size and
scaling. Fixed size is an optional policy, not a universal restriction. These
choices should adjust gracefully to the available display while leaving large-map
camera scrolling independent. Configuration can change the window and transform
policies without rewriting protocol snapshots; it is not implemented in this
phase. The current policy starts maximized, remains restorable and resizable, and
uses uniform downscaling with a 1× cap.

## Opt-in input latency diagnostics

`ICHILOTO_GPUI_TRACE=1` enables NDJSON observations on **stderr only**. It is off by
default; stdout event schemas are unchanged. Each key has one session-local ID,
GPUI `is_held`, and monotonic nanoseconds from a process-local epoch at:

1. `native`: first operation in the GPUI key-down callback, before key formatting.
2. `normalized`: identity normalization completed (`ignored` for rejected keys).
3. `queued`: timestamp immediately before a successful nonblocking writer submission.
4. `write_started`: writer dequeued the event and is about to serialize/write it.
5. `written`: the complete stdout line and flush both succeeded.

The producer's `pending_before_enqueue` and consumer's `pending_after_dequeue` are
separate channel-length snapshots, excluding the in-flight write. They must not be
added together or treated as an atomic outstanding count. `in_flight` marks the
writer start/completion. The analyzer reconstructs submission-through-flush event
lifetimes separately. Correlate by ID and timestamps, not stderr line order: the
consumer can start before the producer records its successful submission.

Diagnostics use a separate bounded 4096-record worker. GPUI and the protocol writer
only attempt nonblocking diagnostic submissions; a slow stderr sink drops records
instead of blocking input. A closing summary reports `dropped_records`; incomplete
or dropped traces cannot substantiate complete timings. Shutdown drains normal
stdout first and allows up to two additional seconds for enabled diagnostics.
Initial sizing and changed viewport geometry are also recorded. Frame lifecycle
records and clock anchors use the same switch, as described below. PHP consumption
and actual GPU/display completion are not measured by this renderer.

```sh
ICHILOTO_GPUI_TRACE=1 php scripts/inspect-fixture.php --binary target/release/gpui-renderer --geometry last-legend \
  > /tmp/events.ndjson 2> /tmp/trace.ndjson
php scripts/analyze-trace.php /tmp/trace.ndjson --events /tmp/events.ndjson
# Select an inclusive ID interval or only events GPUI flags as held:
php scripts/analyze-trace.php /tmp/trace.ndjson --ids 3:12
php scripts/analyze-trace.php /tmp/trace.ndjson --held
```

On pinned GPUI 0.2.2 macOS, nonprinting keys can pass through
`do_command_by_selector`, which reconstructs their key-down event with
`is_held=false`. The physical Right-hold validation encountered this limitation:
59 events arrived at a repeat cadence, but every flag was false. The logger retains
the supplied flag unchanged; `--held` cannot identify that interval. Use the
user-confirmed interval's IDs instead of inferring taps from a false flag.

See [viewport and latency validation](docs/viewport-latency-validation.md) for native
measurements and evidence. Callback-to-flush timing does not measure OS-to-GPUI
delivery, PHP consumption, frame presentation or end-to-end game responsiveness.
Key-repeat semantics remain unchanged: every accepted GPUI key-down is forwarded.

## Clock alignment and frame diagnostics

With tracing enabled, macOS emits a `clock_anchor` at startup and shutdown. Each
record contains `clock:"CLOCK_UPTIME_RAW"`, the renderer `pid`, `host_ns`, and
`elapsed_before_ns`/`elapsed_after_ns` bracketing that host-clock read. The Darwin
`clock_gettime_nsec_np` API and libc's `clockid_t`/`CLOCK_UPTIME_RAW` definition are
used directly. The development host's PHP build was verified to use the same clock
for `hrtime(true)`. Other platforms emit `clock_anchor_unavailable`; they do not
pretend to share that clock. Missing host-clock alignment does not prevent native
window startup or process-relative diagnostics on those platforms.

To align an event's process-relative `at_ns` to the same host clock, use integer
nanosecond arithmetic and retain the uncertainty interval:

```text
offsetLow  = host_ns - elapsed_after_ns
offsetHigh = host_ns - elapsed_before_ns
eventHost  ∈ [at_ns + offsetLow, at_ns + offsetHigh]
```

Check startup/shutdown anchor consistency and the clock used by the other process.
These anchors do not synchronize separate machines or arbitrary PHP/platform clocks.
Raw `Instant` epochs from different processes must never be subtracted directly.

Every parsed frame receives an optional renderer-local `sequence`, distinct from
its source `frame` number. The trace record also includes `protocol`, `stage` and
monotonic `at_ns`. Source numbers can repeat or decrease; sequence correlates that
particular received snapshot through these stages:

| Stage | Observation boundary |
| --- | --- |
| `received` | Complete bounded NDJSON line read, before parsing; emitted only when parsing identifies a frame |
| `accepted` | Session/frame validation and asset preparation succeeded, before submitting the prepared update |
| `submit_begin` / `submit_end` (or `submit_failed`) | Around the existing bounded blocking send; includes instantaneous queue count/capacity |
| `dequeued` | Foreground receiver obtained the update; includes queue count/capacity |
| `replace_begin` | Immediately before snapshot assignment |
| `replaced` | GPUI app handler replaced the current prepared snapshot, before requesting repaint |
| `render_callback` | Renderer callback entry for the current snapshot, before constructing elements |
| `elements_built` | Root element subtree constructed, before returning it to GPUI |
| `layout_request_begin/end` | Root's public Element request-layout method; excludes later layout computation |
| `prepaint_begin/end` | Root's public Element prepaint method; layout computation can occur in the preceding gap |
| `paint_begin/end` | Root's public Element CPU paint method; excludes subsequent native presentation |

Receipt-to-acceptance includes parsing, validation and preparation. Acceptance-to-
replacement includes the bounded reader queue, UI scheduling and device-density
glyph raster preparation when effects are present; startup also includes native
window creation. `accepted` does not establish glyph admission: that later step
can reject a candidate before `replace_begin`, preserving the previous display.
Previous foreground rendering can delay the next
update. The opt-in root observer delegates the same element ID, state types, bounds
and arguments; disabled tracing uses the original unwrapped element.

Queue counts are instantaneous observations, not an atomic history. `submit_end`
may be recorded after the consumer dequeues/replaces the frame. Match sender
begin/end to measure submission; use dequeue to locate foreground handling.
Pair every chronological begin/end occurrence separately, since a frame can be
rendered multiple times. Do not merge first/last stages across callbacks.

A render callback or `paint_end` is **not GPU completion or proof
of visible pixels**. GPUI can coalesce intermediate snapshots and repaint the same
snapshot more than once. Interpret missing stages alongside protocol errors,
shutdown/capture boundaries and dropped-record counts, not as automatic evidence
of a lost frame. Malformed lines without a parsed frame have no frame trace.

Observation context is internal Rust metadata, never a protocol field, frame
acknowledgment or gameplay identifier. It is absent when tracing is disabled.
Diagnostics are serialized into complete lines on the worker; stderr can also
contain ordinary error messages. The key analyzer reports those separately and
rejects malformed diagnostic lines. See [frame diagnostic validation](docs/frame-diagnostics-validation.md)
for paired PHP clock probes, native lifecycle traces, screenshots and limitations.

Asset paths canonicalize beneath canonical `assetRoot`. Absolute replacements,
paths and symlinks escaping the root are rejected. In-root parent traversal/symlinks
can resolve successfully. Files must decode as PNG regardless of suffix. The root
and filesystem are trusted host configuration; concurrent hostile filesystem mutation
is outside the contract. PNGs decode off the UI thread. A session LRU cache reuses
full decoded sheets across frames, keyed by canonical path and file length/mtime.
Metadata changes reload the image; same-length edits with preserved timestamps
require a new session. Choose a suitably narrow asset root.

The cache holds at most 64 MiB / 1024 images. Prepared snapshots retain their cache
generation while queued or displayed; these retained generations and the in-progress
decode are additional bounded memory, not part of a process-wide 64 MiB promise.
Old atlas entries are retired when the UI draws a newer cache generation, preserving
the previous visible scene until its replacement is drawn. Crop-only changes reuse
the same full-sheet image identity and GPUI atlas upload.

| Resource | Maximum |
| --- | --- |
| NDJSON line, including newline when present | 4 MiB |
| Grid | 512 columns × 256 rows |
| Cell | 256 × 256 logical pixels, positive |
| Grid extent | 16384 logical pixels per axis |
| Sprites per frame | 1024 |
| Tile batches / cells per frame | 64 / 32768, independent of actors |
| Tile source catalog | 1..256 per batch, 4096 per frame |
| PNG source and displayed dimensions | 4096 × 4096 |
| Encoded PNG | 16 MiB |
| Decoded source images per frame (actors + tiles) | 64 MiB / 1024 images |
| Session decoded-image cache | 64 MiB / 1024 images |
| Prepared tile regions per frame and in session LRU | 64 MiB / 4096 regions, including guards |
| Shared tile/canvas/glyph display-raster LRU | 64 MiB / 32768 images; evictions retire at the next render |
| V2 text layers | 64 |
| V2 runs across all layers | 32768 |
| V2 Unicode scalars across all runs, including spaces/overlap | 524288 |
| V2 text layer ID | 256 UTF-8 bytes |

Validation, PNG decoding and source-region preparation happen before displayed-state
mutation. Display samples are derived during paint, when device scale and final
surface position are known. Guarded canvas images, composites and effected text
use the same device-pixel sampling path as terrain. Their clip and destination
stay independent of source resolution: switching from a full-size source to a
smaller composite cannot round its borders into a different position. Clipping
changes coverage without changing sample coordinates. This uses the existing
shared display LRU and retirement mechanism, and adds canvas resampling work to
native drawing; the CPU preparation timings above exclude that work.
Samples evicted while painting
remain alive until the next render boundary so their GPU slots cannot be reused
under the current scene. These in-flight samples and GPUI uploads are additional
memory, not part of a process-wide cache limit. Glyph candidate admission counts
live tile/canvas/glyph rasters held by snapshots and retirement queues, new candidate
rasters and peak mask/blur scratch against that same 64 MiB display allowance;
eviction does not make retained bytes disappear. There is no second glyph pool.
Glyph raster dimensions are bounded to 16384 device pixels per axis, and a bounded
cache-miss work check applies before preparation. These are native resource checks,
not increases or replacements of the wire layer/run/scalar limits.

## Key identities

```json
{"protocol":2,"type":"key","key":"C"}
```

Supported output identities:

- Every ASCII letter `a`..`z` and `A`..`Z`.
- `up down left right enter space escape tab shift_tab backspace delete insert`.
- `home end page_up page_down`.
- `f0` through `f20`, where the platform exposes them.
- `do find help next previous select`, if GPUI supplies those exact standalone names.

GPUI's actual `pageup`/`pagedown` names become `page_up`/`page_down`. `tab` plus
Shift becomes `shift_tab`. For letters, matching `key_char` preserves reported case;
otherwise Shift uppercases the letter and the unshifted identity is retained.
This supports reported caps-lock case without guessing keyboard layout.

Control, Alt and platform (Command/Super) modified keys are ignored, including
Command-W, Ctrl-C and Alt-X: they never degrade to plain letters. No general chord
schema is introduced. Fn-modified ordinary letters are ignored; an explicit standalone
special key or F-key can pass even when GPUI retains its function modifier. GPUI
0.2.2 macOS normally removes that modifier for the native special-key range, and maps
Shift-Tab to `key:"tab"` with Shift. The normalizer follows the actual GPUI API,
not browser `KeyboardEvent` names. Shift on other named keys adds no chord identity.

OS repeats are forwarded; key-up events are not. The renderer never emits actions
such as `cancel`, `map`, `move_up`, `confirm` or `quit`. **PHP owns input bindings.**
Historical native validation covered C/M/T/Tab/Shift-Tab/F5 and legacy keys.
F0, Insert and uncommon keys still depend on platform availability; see the explicit
native-evidence limitations in the validation document.

## Lifecycle

| Direction | Type | Behavior |
| --- | --- | --- |
| In | `shutdown` | Selected protocol; drain output and exit 0, without close event |
| Out | `ready` | Native window successfully initialized |
| Out | `close_requested` | Native close; drain output and exit 0 |
| Out | `error`, with `message` | Recoverable rejection retains display; fatal errors exit nonzero |

Each object includes its selected numeric `protocol` and string `type`. Shutdown
before hello is allowed for protocol 2 and emits nothing. EOF after
hello keeps the window and keyboard active. EOF before a successful hello is fatal.
Broken/full stdout or window-creation failure is fatal. Shutdown allows two seconds
to drain output; an unavailable output channel can only report diagnostics to stderr.

## Native fixtures

The original `scripts/fixture.php` and `scripts/inspect-fixture.php` emit
stateless protocol 1 or 2 frames and remain historical calibration fixtures;
they cannot drive the production retained renderer. Use the retained replay tool
above with an NDJSON capture for native presentation inspection, or the Engine's
retained fixture smoke for a small controlled window. Both are fixture-only and
silent. Check existing processes first and keep only one inspection window open.

## Implementation boundaries

`protocol.rs` has separate typed v1/v2 frame DTOs and versioned event serialization;
`color.rs` validates tagged colours and maps the palette. `transport.rs` validates
hello/session ordering on a bounded background stdin reader. `assets.rs` safely loads
PNGs. `state.rs` builds the immutable prepared snapshot, explicit opaque cells and
stable `PaintItem` plan; v1 gets a dedicated legacy-text item before all sprites.
`renderer.rs` paints that plan with the shared `viewport.rs` transform and fixed cell
rectangles. `window_layout.rs` selects initial native bounds. `input.rs` normalizes
keys; `diagnostics.rs` optionally observes the key path; `app.rs` owns the one window
and lifecycle.

The UI never reads stdin, decodes PNGs or writes pipes. Queues are bounded at two
input updates and 256 output events. Every queued event carries its own version, so
buffered pre-session errors cannot be relabelled after a successful hello. There is
no equality-based redraw suppression; all accepted state (IDs, layers, runs, styles,
sprites) is replaced and notified intact. The only renderer timer is shutdown drain.

Engine uses v2 with negotiated graphical capabilities for GPUI; v1 remains a
compatibility path. PHP supplies structured colors without ANSI, preserves
explicit blank UI cells and chooses layering and transient timing.
The renderer does not infer missing UI backgrounds, masks, overlays or game bindings.
