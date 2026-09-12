# Ichiloto GPUI renderer

A standalone native presentation and keyboard surface using pinned **GPUI 0.2.2**.
It supports protocol v1 and v2 sessions. PHP owns actions, input bindings, movement,
collision, scenes, battle state, camera conversion, timing and saves. Rust receives
complete presentation snapshots and forwards key identities. It has no game loop,
audio, ANSI parser, terminal emulator or gameplay meaning for layer IDs/numbers.

## Build and validate

Rust **1.98.1** is pinned in `rust-toolchain.toml`; commit and use `Cargo.lock`.

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked
python3 scripts/native-smoke.py
```

The native smoke suite opens **sequential silent windows** on a graphical desktop.
It tests both versions and IPC/lifecycle failures; it does not synthesize keyboard
proof or assert visual pixels. Use the interactive fixture below for those checks.
Optional `--binary PATH` selects an executable, and `--evidence-dir PATH` records
per-case stdout, stderr and exit status.

Validated on macOS/Apple Silicon, with Xcode/SDK/Metal toolchain installed.
Initial usable-area fitting is currently implemented on macOS. Other targets keep
the platform-neutral paint model but explicitly reject native window creation until
a work-area adapter exists; GPUI 0.2.2's full display bounds are insufficient for
that guarantee. Cross-target compilation has not been validated here. Upstream
`block 0.1.6` and `proc-macro-error2 2.0.1` report future-compatibility warnings;
current builds, Clippy and tests pass. See [S7-R validation](docs/s7-r-validation.md)
and the historical [S1 validation](docs/s1-validation.md).

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

For v2, text layers and sprites share one ascending numeric order:

1. Lower layers paint before higher layers.
2. Equal-layer text layers paint first, preserving their frame array order.
3. Equal-layer sprites paint second, preserving their frame array order.

Use distinct layers for specific cross-type occlusion. Values such as world `0`,
sprite `100` and UI `1000` are caller policy, never hardcoded gameplay rules.
`textLayers:[]` removes all previous text layers. Together with `sprites:[]`, it
clears the entire snapshot to the default surface background.

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

ANSI16 uses this deterministic VGA-style palette. ANSI256 indices 0..15 reuse it:

| Index | RGB | Index | RGB |
| --- | --- | --- | --- |
| 0 | #000000 | 8 | #808080 |
| 1 | #800000 | 9 | #FF0000 |
| 2 | #008000 | 10 | #00FF00 |
| 3 | #808000 | 11 | #FFFF00 |
| 4 | #000080 | 12 | #0000FF |
| 5 | #800080 | 13 | #FF00FF |
| 6 | #008080 | 14 | #00FFFF |
| 7 | #C0C0C0 | 15 | #FFFFFF |

ANSI256 indices 16..231 use the standard xterm 6×6×6 cube with component levels
`[0,95,135,175,215,255]`: for `n=index-16`, red index `n/36`, green `(n/6)%6`,
blue `n%6` (integer division). Indices 232..255 are grayscale `8+10*(index-232)`.

## Sprite geometry and safety

The sprite DTO is unchanged across versions: nonempty unique `id`, relative PNG
`asset`, signed 32-bit cell coordinates `x/y`, positive logical pixel `width/height`,
`anchor:"bottom_center"` and signed 32-bit `layer`. Off-grid sprites are valid and
clipped to the presentation surface. No source rectangles, tint, effects or animation.

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

## Logical grid and native viewport

The hello fixes the logical surface for the entire session:
`logicalWidth = columns * cellWidth`, `logicalHeight = rows * cellHeight`.
Native window dimensions are independent. Every paint uses the actual
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
cell backgrounds, sprite origins/dimensions and bottom-center anchors share this
transform. The inner surface clips off-grid sprites at the logical grid boundary;
the outer viewport paints the default background. There are no scrollbars or
independent X/Y scaling. Resizing requests a GPUI repaint; it never modifies stored
frames, negotiates a grid, sends protocol resize events or changes gameplay/camera
coordinates. A zero-sized viewport paints no surface and retains the focus root.
Large maps can extend beyond this fixed logical viewport: PHP's camera scrolls the
visible portion and sends new snapshots. The renderer does not fit an entire map
unless its caller deliberately sends the entire map as the logical surface.

On macOS, `window_layout.rs` reads `NSScreen.mainScreen` (first screen fallback),
`frame`, `visibleFrame` and `NSScreenNumber`. It matches that display to GPUI and
passes its explicit `display_id` when opening. AppKit's
`contentRectForFrameRect:styleMask:` and `frameRectForContentRect:styleMask:` account
for the actual normal resizable titlebar. Fitted content is selected first, capped
at 1× and rounded inward to whole points, then its outer frame is centered in the
usable work area. The top-left is aligned to whole points to prevent AppKit from
rounding fractional initial rectangles outward. Centering may differ by less than
one point due to this alignment. No screen dimensions, Dock/menu sizes or chrome
constants are guessed.

Pinned GPUI's `PlatformDisplay` exposes no usable-area API, and `MacDisplay::bounds`
discards global display origins. The adapter therefore converts AppKit's global
bottom-left coordinates to the display-relative outer top-left expected by pinned
`MacWindow::open`. Tests cover positive and negative monitor origins. Native
validation currently covers the development display, not a physical multi-monitor
setup. Direct macOS dependencies reuse the already locked cocoa/objc versions;
no GPUI or registry source is patched.

The logical size, initial native-size policy, actual viewport and paint transform
are separate boundaries. Future preferred scale/size, explicit upscale or fullscreen
configuration can change the window policy and transform policy without rewriting
protocol snapshots. Such configuration is not implemented in this phase.
An explicit fixed native-window policy could fit a preferred size and disable
resizing while leaving camera scrolling intact; the current policy is resizable.

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
Initial sizing and changed viewport geometry are also recorded, without changing
frame state. No frame-receive, frame-paint or PHP timings are measured by this switch.

```sh
ICHILOTO_GPUI_TRACE=1 python3 scripts/inspect-fixture.py --geometry last-legend \
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

Asset paths canonicalize beneath canonical `assetRoot`. Absolute replacements,
paths and symlinks escaping the root are rejected. In-root parent traversal/symlinks
can resolve successfully. Files must decode as PNG regardless of suffix. The root
and filesystem are trusted host configuration; concurrent hostile filesystem mutation
is outside the contract. PNGs decode off the UI thread and are reused within a frame;
persistent cross-frame caching is deferred. Choose a suitably narrow asset root.

| Resource | Maximum |
| --- | --- |
| NDJSON line, including newline when present | 4 MiB |
| Grid | 512 columns × 256 rows |
| Cell | 256 × 256 logical pixels, positive |
| Grid extent | 16384 logical pixels per axis |
| Sprites per frame | 1024 |
| PNG source and displayed dimensions | 4096 × 4096 |
| Encoded PNG | 16 MiB |
| Decoded images per frame | 64 MiB |
| V2 text layers | 64 |
| V2 runs across all layers | 32768 |
| V2 Unicode scalars across all runs, including spaces/overlap | 524288 |
| V2 text layer ID | 256 UTF-8 bytes |

Validation and complete image preparation happen before displayed-state mutation.

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
python3 scripts/fixture.py | target/debug/gpui-renderer
# V2 first visual frame; --frame second selects the changed style snapshot.
python3 scripts/fixture.py --protocol 2 --frame first | target/debug/gpui-renderer
# Controlled single-window fixture, with protocol logs captured separately.
python3 scripts/inspect-fixture.py > /tmp/events.ndjson 2> /tmp/renderer.log
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

The Engine still sends v1 today. Renderer support alone does not restore game
colours, UI layering or transient timing; those require the separate S7-E work.

S7-E must opt into a v2 hello and v2 DTOs consistently, produce structured colours
without ANSI, preserve explicit blank UI cells, and choose layer policy in PHP.
The renderer does not infer missing UI backgrounds, masks, overlays or game bindings.
