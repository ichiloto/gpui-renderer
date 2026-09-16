# Ichiloto GPUI renderer

The negotiated v2 `graphical_canvas` extension draws images at explicit graphical
rectangles, with outlines/underlines and locally positioned text. PHP resolves
image pivots, contain-fit geometry and presentation state. Native resizing fits
the complete canvas uniformly and centers it. Existing field sprite/tile grid
coordinates retain their meaning.

Project-owned image cursors use the existing canvas image path; see
[cursor presentation](#cursor-presentation) for ownership and limits.

The [G1 wire corpus](fixtures/graphical-canvas/manifest.json) and its
[hash manifest](fixtures/graphical-canvas/SHA256SUMS) freeze the shared boundary.
Each accepted canvas fully replaces the previous one; omitting it clears all
canvas content and resumes the legacy frame. Canvas text uses transparent null
backgrounds, while explicit colors paint opaque cells. Legacy text backgrounds
are unchanged. Canvas and nonempty legacy collections cannot share a frame.

The additional v2 `canvas_clip_opacity` capability requires `graphical_canvas` in
the same hello and acknowledgement in ready. It adds optional `clipRect` to canvas
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
`graphical_canvas`. Omitting the fields on a later full frame removes the previous
clip/fade. Source crops, caches, budgets and legacy text behavior are unchanged;
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

Run `scripts/native-tile-smoke.py --canvas --binary <installed-executable>
--evidence-dir <directory>` for one silent native check. Optional
`--observe-seconds 45` holds its first frame for inspection, then closes the
owned window automatically. This synthetic check is not real-game acceptance.

A standalone native presentation and keyboard surface using pinned **GPUI 0.2.2**.
It supports protocol v1 and v2 sessions. PHP owns actions, input bindings, movement,
collision, scenes, battle state, camera conversion, timing and saves. Rust receives
complete presentation snapshots and forwards key identities. It has no game loop,
audio, ANSI parser, terminal emulator or gameplay meaning for layer IDs/numbers.

## Availability and installation

This section is the current installation status; dated validation documents and
receipts describe their original checkpoints, including superseded binaries.
On 16 September 2026, the accepted implementation from local commit
`b44b08a8393ab74c28a0ff5c947274c5cbfca4f1` was integrated through `develop` into
`main`. Its existing optimized executable, SHA256
`8f946ccac39d1f6aa50e1edb4712300197ad6bfcea361b221dac060f3a52bbc5`, was installed
in the normal Engine package at
`resources/renderers/installed/gpui/darwin-arm64/Ichiloto Renderer.app/Contents/MacOS/gpui-renderer`.
Engine preserved its original app metadata and manifest and verified the package
resolver. This reused the tested binary without another build or runtime copy.

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

Rust **1.98.1** is pinned in `rust-toolchain.toml`; commit and use `Cargo.lock`.

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
python3 scripts/native-smoke.py --binary target/release/gpui-renderer
```

For a single silent window checking both negotiated capabilities, full-viewport
terrain and frame clearing, run `scripts/native-tile-smoke.py` with `--binary`
and `--evidence-dir`. See the [installed-renderer check](docs/s8-b-installation.md).

Ordinary gameplay uses Engine's installed optimized executable. When a new build
is necessary, `target/release/gpui-renderer` is the packaging and performance
validation input. `cargo build --locked` produces an unoptimized
`target/debug/gpui-renderer` for development/debugging; it is not the performance
baseline. A matched Last Legend investigation found long foreground paint work in
the debug build and substantially shorter frame handoff/draw intervals with the
same source compiled in release mode. This is a build-profile comparison, not a
scheduling or gameplay change; see [the measured investigation](docs/burst-investigation.md).
Packagers should preserve the native bundle/resources and stage the optimized
executable through their existing installation boundary, verifying its hash.

Engine's GPUI launch now retains the shared presentation buffer while skipping
physical terminal drawing. The [Garden of Roads comparison](docs/garden-performance.md)
records that improvement, the shared style-processing fix, and validation limits.

The native smoke suite opens **sequential silent windows** on a graphical desktop.
It tests both versions and IPC/lifecycle failures; it does not synthesize keyboard
proof or assert visual pixels. Use the interactive fixture below for those checks.
Optional `--binary PATH` selects an executable, and `--evidence-dir PATH` records
per-case stdout, stderr and exit status. `--trace` validates enabled diagnostic
stderr separately from ordinary errors while retaining the same stdout assertions;
the default smoke run explicitly disables tracing.

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
The accepted source passed 106 optimized tests, Clippy and the recorded native
checks; see [glyph-effects validation](docs/glyph-effects-validation.md), the
earlier [S7-R validation](docs/s7-r-validation.md) and [S1 validation](docs/s1-validation.md).

## Session and channel contract

One UTF-8 JSON object per NDJSON line. **stdin is protocol input only; stdout is
protocol output only; stderr is diagnostics only.** No ANSI sequences or arbitrary
CSS colours are accepted. Required fields are typed; unknown fields, message types,
anchors and unsupported versions are rejected. Protocol messages use `protocol:1`
or `protocol:2`; one successful hello selects the version for the entire process.

```json
{"protocol":2,"type":"hello","title":"Last Legend","assetRoot":"/absolute/path/to/assets","grid":{"columns":135,"rows":36,"cellWidth":10,"cellHeight":20}}
```

The hello shape is identical in both versions. Dimensions must satisfy the bounds
below. `assetRoot` must be absolute and canonicalize to an existing directory; title
must be nonempty without control characters. A successful hello opens one resizable
native window, then emits `ready`. The version is latched before opening it, so even
a window-creation error uses that version. A second hello cannot open another window.

Sprite-sheet cropping is explicitly negotiated on either version. Add
`"requiredCapabilities":["sprite_source_rect"]` to hello. A successful opted-in
ready includes `"capabilities":["sprite_source_rect"]`; the caller must verify
that acknowledgment before sending crops. Missing or empty requirements keep the
legacy ready shape, with no capabilities field. Unknown or duplicate requirements
are rejected. Older renderers reject the new hello field; callers must also reject
a missing acknowledgment, never silently fall back to drawing the whole sheet.

Graphical terrain uses the v2-only `tile_batches` capability. Automatic Engine
startup requests both capabilities before any later map transfer. Every
`tileBatches` field, including an empty array, requires acknowledgment; v1
rejects this capability. See the [frozen tile contract](docs/tile-batches.md).

Before a successfully validated hello, errors use the legacy `protocol:1` envelope,
**including errors for invalid v2 hellos**. That is a pre-session exception, not
negotiation or downgrade. Once initialized, every `ready`, `key`, `close_requested`
and `error` uses the selected version. Mixed-version frames, hellos and shutdowns
are errors; they do not replace the display, reinitialize or stop the session.

Frames require a successful hello. Each accepted frame atomically replaces the
entire previous snapshot. A malformed frame, invalid text or failed PNG load leaves
the last accepted display unchanged. Labels are unsigned 64-bit integers, not clocks:
duplicate/decreasing labels apply in arrival order. There is no frame acknowledgement,
deduplication, interpolation or renderer timing. Every accepted frame notifies GPUI,
including colour-only updates with unchanged glyphs or labels.

## Protocol v1 compatibility

The v1 frame schema and historical drawing order remain intact:

```json
{"protocol":1,"type":"frame","frame":1,"text":["row one","row two"],"sprites":[{"id":"player","asset":"South.png","x":8,"y":4,"width":32,"height":48,"anchor":"bottom_center","layer":100}]}
```

Text uses the default colours. Missing rows/trailing cells are blank; `text:[]`
clears all text, and `sprites:[]` clears all sprites. Overlong rows, too many rows
and control characters are rejected. One Unicode scalar occupies one fixed cell.
The complete text plane paints first, then sprites sorted by ascending layer with
stable array order on ties. Even a sprite at `i32::MIN` paints after v1 text.
V2 fields such as `textLayers` are not accepted in v1 frames.

## Protocol v2

```json
{
  "protocol": 2,
  "type": "frame",
  "frame": 1,
  "textLayers": [
    {"id":"world","layer":0,"runs":[
      {"row":4,"column":2,"text":"Hello","foreground":{"kind":"ansi16","index":14},"background":null}
    ]},
    {"id":"ui","layer":1000,"runs":[
      {"row":4,"column":8,"text":"     ","foreground":null,"background":null}
    ]}
  ],
  "sprites": [
    {"id":"player","asset":"South.png","x":8,"y":4,"width":32,"height":48,"anchor":"bottom_center","layer":100}
  ]
}
```

All shown frame/layer/run fields are required; empty arrays are allowed. Layer IDs
are nonempty UTF-8 strings, unique among text layers in a frame, at most 256 UTF-8
bytes. They are presentation identities only. Sprite IDs have their existing
separate uniqueness domain. Layers are signed 32-bit integers.

Runs contain a zero-based `row`, zero-based starting `column`, UTF-8 `text`, and
required nullable `foreground`/`background`. Omission is malformed; explicit null
means the renderer default. Every Unicode scalar advances exactly one grid cell,
independent of byte length, glyph width or font advance. Grapheme clustering and
terminal wide-character continuation cells are not inferred. Controls, tabs and
newlines are rejected. The complete run must fit; invalid runs are never clipped.
An empty run paints nothing, but its row and column must still be inside the grid.

**Every covered cell is opaque, including spaces.** Its background rectangle uses
the explicit colour or default background. A non-space scalar paints over that
rectangle using the explicit foreground or default text colour. A space paints just
the background. Cells absent from runs are transparent to lower layers. Overlapping
runs execute in their array order, so later cells replace earlier cells.

For v2, tile batches, text layers and sprites share one ascending numeric order:

1. Lower layers paint before higher layers.
2. Equal-layer tile batches paint first, preserving batch and cell array order.
3. Equal-layer text layers paint next, preserving their frame array order.
4. Equal-layer sprites paint last, preserving their frame array order.

Use distinct layers for specific cross-type occlusion. Values such as world `0`,
sprite `100` and UI `1000` are caller policy, never hardcoded gameplay rules.
`textLayers:[]` removes all previous text layers. Together with `sprites:[]` and
omitted or empty `tileBatches`, it
clears the entire snapshot to the default surface background.

### Tile batches

With `tile_batches` negotiated, v2 frames may include:

```json
"tileBatches":[{"id":"terrain","asset":"Field.png","layer":-100,"sources":[{"x":0,"y":0,"width":16,"height":32}],"cells":[{"column":8,"row":3,"source":0}]}]
```

Cells are already Camera-projected, unsigned grid coordinates. Each fills one
session cell; `source` indexes image-pixel rectangles in the batch catalog.
There is no map interpretation or terrain animation in Rust. Omission and `[]`
clear previous tiles; explicit null is invalid. A malformed batch, invalid crop
or budget failure rejects the entire frame, preserving the accepted display.

Terrain has an independent 32768-cell budget and one canvas/layout element per
batch, while each cell still emits an image primitive. Guarded source regions
are prepared and cached for reuse, with separate byte/count accounting; they
isolate neighbouring atlas tiles. Cached device-pixel samples compensate for
GPUI's image-bound rounding, preserving the selected artwork's full mapping at
fractional scales and positions. Unchanged samples are reused across cells/frames.
See [S8-B implementation and validation](docs/s8-b-validation.md) and the
[shared exact wire fixtures](fixtures/tile-batches/manifest.json).

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
visible portion and sends new snapshots. The renderer does not fit an entire map
unless its caller deliberately sends the entire map as the logical surface.

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
ICHILOTO_GPUI_TRACE=1 python3 scripts/inspect-fixture.py --binary target/release/gpui-renderer --geometry last-legend \
  > /tmp/events.ndjson 2> /tmp/trace.ndjson
python3 scripts/analyze-trace.py /tmp/trace.ndjson --events /tmp/events.ndjson
# Select an inclusive ID interval or only events GPUI flags as held:
python3 scripts/analyze-trace.py /tmp/trace.ndjson --ids 3:12
python3 scripts/analyze-trace.py /tmp/trace.ndjson --held
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
| Shared tile/glyph display-raster LRU | 64 MiB / 32768 images; evictions retire at the next render |
| V2 text layers | 64 |
| V2 runs across all layers | 32768 |
| V2 Unicode scalars across all runs, including spaces/overlap | 524288 |
| V2 text layer ID | 256 UTF-8 bytes |

Validation, PNG decoding and source-region preparation happen before displayed-state
mutation. Display samples are derived during paint, when device scale and final
cell position are known. They have a separate LRU; samples evicted while painting
remain alive until the next render boundary so their GPU slots cannot be reused
under the current scene. These in-flight samples and GPUI uploads are additional
memory, not part of a process-wide cache limit. Glyph candidate admission counts
live tile/glyph rasters held by snapshots and retirement queues, new candidate
rasters and peak mask/blur scratch against that same 64 MiB display allowance;
eviction does not make retained bytes disappear. There is no second glyph pool.
Glyph raster dimensions are bounded to 16384 device pixels per axis, and a bounded
cache-miss work check applies before preparation. These are native resource checks,
not increases or replacements of the wire layer/run/scalar limits.

## Key identities (both versions)

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
This expansion also applies to v1; existing v1 key identities are unchanged.
Native validation covers the required C/M/T/Tab/Shift-Tab/F5 and legacy keys.
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
before hello is allowed for either supported version and emits nothing. EOF after
hello keeps the window and keyboard active. EOF before a successful hello is fatal.
Broken/full stdout or window-creation failure is fatal. Shutdown allows two seconds
to drain output; an unavailable output channel can only report diagnostics to stderr.

## Native fixtures

```sh
# Existing v1 fixture; EOF leaves the native window open.
python3 scripts/fixture.py | target/release/gpui-renderer
# V2 first visual frame; --frame second selects the changed style snapshot.
python3 scripts/fixture.py --protocol 2 --frame first | target/release/gpui-renderer
# Controlled single-window fixture, with protocol logs captured separately.
python3 scripts/inspect-fixture.py --binary target/release/gpui-renderer > /tmp/events.ndjson 2> /tmp/renderer.log
```

For the controlled fixture, type commands into its launching terminal: `second`
changes colours without changing text; `first` restores; `invalid` attempts a missing
asset; `clear` empties the snapshot; `shutdown` exits via the protocol. Use the native
window for actual keyboard testing. Native close also ends the driver. `--protocol 1`
selects the original Home calibration fixture. `--binary PATH` supports a local app
bundle executable. `--geometry last-legend` selects 135×36 at10×20 and adds a complete
grid border; `--geometry oversized` selects 135×36 at20×40 (2700×1440 logical).
`--geometry oversized-tall` selects 135×36 at20×80 to exercise height/chrome fitting.
The unused area is intentional; these are alignment fixtures, not game screens.
All fixture assets belong to this repository; no Ichiloto/PHP,
game checkout, game process or audio is used. Check existing processes first and
keep only one interactive fixture window open.

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
