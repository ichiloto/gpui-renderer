# Ichiloto GPUI renderer — S0/S1

A standalone native presentation and keyboard surface for Ichiloto, using the
published [`gpui = "=0.2.2"`](https://docs.rs/gpui/0.2.2/gpui/) crate.
**Ichiloto remains the PHP game engine. Phase S1 does not yet communicate with PHP.**
This process owns only a text grid, complete presentation snapshots, PNG images,
and normalized key events. Gameplay, actions, camera transforms, state, and timing
remain PHP responsibilities. There is no simulation loop.

## Build and test

Rust **1.98.1** (stable on 2026-09-11) is pinned in `rust-toolchain.toml`.
Cargo's lockfile is committed. Install Rust through [rustup](https://rustup.rs/), then:

```sh
cargo build --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

With a graphical desktop available, `python3 scripts/native-smoke.py` also runs
seven real-process IPC/lifecycle checks. It opens brief native windows. Visual
alignment and actual keyboard input still require the interactive fixture check.

The validated native platform for S1 is **macOS on Apple Silicon**. GPUI requires
Xcode and its macOS SDK/Metal compiler. If compilation reports a missing Metal
Toolchain, install the official component with `xcodebuild -downloadComponent MetalToolchain`.
The Xcode developer directory must be selected (`xcode-select -p`). See
[GPUI's published requirements](https://docs.rs/gpui/0.2.2/gpui/#dependencies).
GPUI also supports Linux; its system dependencies and native behavior have not
been validated here. No separate `gpui_platform` crate is required: the published
`gpui` crate provides `Application::new()` and the platform implementation.

## Run the fixture

From this repository, after building:

```sh
python3 scripts/fixture.py | target/debug/gpui-renderer
```

Python 3 is only a fixture convenience; the renderer itself has no Python dependency.
The script emits `fixtures/home-frame.ndjson`, replacing its absolute sentinel
`/__ICHILOTO_FIXTURES__` with this checkout's absolute `fixtures` directory. It makes
the committed fixture portable without changing the protocol's absolute-root rule.
For direct stdin playback, materialize it first:

```sh
python3 scripts/fixture.py > /tmp/ichiloto-home.ndjson
target/debug/gpui-renderer < /tmp/ichiloto-home.ndjson
```

The window remains interactive after EOF. Press arrows, WASD, shift-WASD, Enter,
Space, Escape, q, and shift-q to see JSON on stdout. These keys do not move anything
or close the window. Click the native close button to emit `close_requested` and exit.
The test PNG is a geometric calibration sprite, not production character art.
Its magenta soles and yellow center make the feet position visible.

Capture channels separately when inspecting IPC:

```sh
python3 scripts/fixture.py | target/debug/gpui-renderer > /tmp/ichiloto-events.ndjson 2> /tmp/ichiloto-renderer.log
```

## Protocol v1

One UTF-8 JSON object per line. Every object has `"protocol":1` and a `"type"`.
All required fields are typed with serde; invalid messages emit an `error` event.
Unknown message types, anchors, unsupported versions, and malformed values are rejected.
Optional future fields are not a supported extension contract in this spike.

First send `hello`:

```json
{"protocol":1,"type":"hello","title":"Last Legend","assetRoot":"/absolute/path/to/assets","grid":{"columns":80,"rows":24,"cellWidth":16,"cellHeight":24}}
```

The root must resolve to an existing directory. Grid/cell dimensions must be positive
integers. Successful initialization opens one native window and emits
`{"protocol":1,"type":"ready"}`. A second hello is rejected; start a new process
to change grid geometry or the root. Frames before hello are rejected.

Each `frame` is a complete, atomic replacement:

```json
{"protocol":1,"type":"frame","frame":1,"text":["row one","row two"],"sprites":[{"id":"player","asset":"Graphics/Characters/Kaelion/Field/South.png","x":8,"y":4,"width":32,"height":48,"anchor":"bottom_center","layer":100}]}
```

- `frame` is an unsigned integer label, not a clock. Frames apply in arrival order;
  duplicate or decreasing labels are accepted. No interpolation or timing is inferred.
- Text rows and x/y coordinates are zero-based. Missing rows and trailing cells are
  blank. Overlong rows, too many rows, tabs, embedded newlines, and control/ANSI
  characters are rejected. S1 places one Unicode scalar per cell; it does not
  implement terminal grapheme/wide-character rules. ASCII is the fixture baseline.
- Every sprite requires a nonempty ID unique within the frame, a relative PNG asset,
  signed integer cell coordinates, positive integer logical-pixel dimensions, an
  anchor enum, and a signed integer layer. Off-grid sprites are clipped.
- `sprites:[]` removes every prior sprite; `text:[]` clears all text. Any validation
  or PNG load failure rejects the entire frame and preserves the last accepted frame.
- Sprites render above text, sorted by ascending layer. Equal layers preserve their
  array order, so later sprites paint on top. Layers only order sprites in S1.

The only anchor is `bottom_center`. With grid origin `(0,0)`:

```text
feetX = (x + 0.5) * cellWidth
feetY = (y + 1.0) * cellHeight
left  = feetX - width / 2
top   = feetY - height
```

At `(8,4)`, 16×24 cells and a 32×48 sprite yield feet **(136,120)** and image
top-left **(120,72)**. Larger sprites never change the grid pitch. These dimensions
are GPUI **logical pixels** (macOS points); GPUI applies the display scale factor
to both text and image output. On a 2× display, one logical pixel occupies two
device pixels. Explicit image dimensions determine display size independently of
the PNG's source resolution. The grid starts at the native window's content origin,
below its titlebar, with no padding.

Supported key events:

```json
{"protocol":1,"type":"key","key":"up"}
```

The vocabulary is `up down left right w a s d W A S D enter space escape q Q`.
GPUI `Keystroke.key`, Shift, and matching `key_char` case normalize these identities;
caps-lock case is preserved where reported by the platform. Control, Alt, platform
(Command/Super), and function-modified chords are ignored because v1 has no chord
schema. OS key-repeat events are forwarded. Key-up events and semantic actions
(`move_up`, `confirm`, `quit`) are not emitted.

Lifecycle messages:

| Direction | Object | Behavior |
| --- | --- | --- |
| In | `{"protocol":1,"type":"shutdown"}` | Drain output and exit 0; no `close_requested` |
| Out | `{"protocol":1,"type":"close_requested"}` | Native window close; drain output and exit 0 |
| Out | `{"protocol":1,"type":"error","message":"..."}` | Recoverable error keeps the last accepted display; fatal errors exit nonzero |

EOF after hello keeps the display and keyboard alive. EOF before successful hello
is fatal. A broken stdout pipe, full output queue, or failed native window creation
is fatal. Output shutdown has a two-second drain deadline to prevent an unread pipe
from hanging the process; an unavailable output channel cannot carry its own error
event, so that diagnostic goes to stderr.

## Architecture and boundaries

`protocol.rs` contains typed wire models and the sole stdout writer abstraction.
`transport.rs` reads stdin on a background thread into a bounded queue, validates
session ordering, and prepares frames. `assets.rs` canonicalizes and loads PNGs;
`state.rs` holds complete presentation snapshots. `renderer.rs` creates the cell
elements and images; `input.rs` normalizes physical/logical key identities.
`app.rs` handles one native window and async lifecycle, with `main.rs` as entry point.

The UI never reads stdin, decodes PNGs, or writes pipes. A two-message input queue
backpressures the reader. A dedicated stdout thread serializes and flushes events
from a 256-event queue; UI sends use `try_send`. Accepted frames trigger `Context::notify()`;
there is no periodic redraw, timer-driven rendering, or game loop. The only timer
is the output drain deadline during shutdown.

Public GPUI APIs used: `Application::new/run`, `App::open_window/spawn/activate/quit`,
`App::on_window_closed`, `WindowOptions`, `WindowBounds`, `Bounds::centered`,
`TitlebarOptions`, `AppContext::new`, `WindowHandle::update`, `AsyncApp::update`,
`Render`, `Context::notify/listener/focus_handle`, `Window::focus/on_window_should_close`,
`div`, `img`, `RenderImage::new/as_bytes/size`, `KeyDownEvent`, `Keystroke`,
`Timer::after`, and public style/layout APIs. Images are decoded into the BGRA order
required by GPUI 0.2.2's `RenderImage`; the UI is given bytes, never asset URLs or
unchecked filesystem paths. No raw wgpu, custom shaders, Zed workspace, or editor
application crates are used.

Asset paths are resolved beneath canonical `assetRoot`. Absolute replacement paths,
parent traversal out of the root, and file/directory symlinks out of the root are
rejected before opening a file. The root and filesystem are trusted host configuration;
concurrent hostile filesystem mutation is outside S1's threat model. PNGs are loaded
off the UI thread and reused within a single frame; persistent cross-frame caching
is deferred. A sender must choose a suitably narrow asset root.

S1 resource limits: 4 MiB per NDJSON line; at most 512×256 cells and 256×256 logical
pixels per cell; at most 16384 logical pixels per grid axis; 1024 sprites per frame;
4096×4096 source/display sprite pixels; 16 MiB encoded PNGs and 64 MiB decoded image
data per frame. These are presentation safety limits, not gameplay rules.

**stdin is protocol input only. stdout is protocol output only, exactly one JSON
object per line. stderr is diagnostics only.** Do not add debugging `println!`
calls to the renderer. The fixture script's stdout intentionally carries protocol input.

## Spike limitations and S2 handoff

Native validation and observations are recorded in [docs/s1-validation.md](docs/s1-validation.md).
GPUI 0.2.2 is pre-1.0; keep the exact pin while reviewing future upgrades.
On macOS, `App::quit()` terminates via Cocoa rather than returning to `main`;
this implementation drains IPC asynchronously and chooses fatal status before
calling it. The published dependency graph reports future-compatibility warnings
for `block 0.1.6` and `proc-macro-error2 2.0.1`; current stable builds/tests work.

S2 should define PHP process supervision, ready/error/close handling, action bindings,
frame production cadence/backpressure, asset caching/invalidation, and any evolution
of modifier or Unicode-cell semantics. PHP must continue to own all of those gameplay
decisions and camera-to-screen conversion. Evaluate other native platforms separately.

S1 has no PHP integration, gameplay, simulation, collision, scenes, maps, quests,
battles, saves, camera logic, tile/NPC graphics, sprite sheets, animation,
interpolation, audio, gamepads, graphical menus, or mouse gameplay.
