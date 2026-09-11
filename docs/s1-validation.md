# S0/S1 acceptance record

Validated 2026-09-11 on Apple Silicon, macOS 26.6.2 (25G83), Rust 1.98.1
(`48a229cea`, 2026-09-01), published GPUI 0.2.2, and Xcode's Metal Toolchain 17F109.
Only `ichiloto/gpui-renderer` was modified. Phase S2 and PHP integration were not started.

## Code and test gates

| Command | Result |
| --- | --- |
| `cargo fmt --check` | Pass |
| `cargo clippy --all-targets --locked -- -D warnings` | Pass |
| `cargo test --locked` | 22 passed, 0 failed, 0 ignored |
| `cargo build --locked` | Pass |
| `git diff --check` | Pass |
| `python3 scripts/native-smoke.py` | All seven real-process checks pass |

The Rust tests cover hello, protocol versions, positive/bounded grid geometry,
frame shape, anchors, shutdown, normalized key serialization and case, complete
replacement/clear semantics, rejected-frame preservation, text bounds/controls,
stable layer order, valid PNG loading/transparency, regular-file/root validation,
absolute/parent/symlink containment, message sequencing, malformed-line recovery,
EOF, and oversized-line recovery.

The native smoke script separately verified:

1. Fixture plus shutdown: complete `ready` line flushed; exit 0.
2. Fixture, malformed line, shutdown: complete `ready` and `error` lines flushed;
   exit 0. Human diagnostics were captured separately on stderr.
3. Shutdown before hello: no output, exit 0.
4. EOF before hello: complete `error` line flushed, exit 1.
5. Unsupported protocol followed by shutdown: complete `error` line, exit 0.
6. Broken stdout: the renderer terminates with exit 1.
7. Unread stdout under sustained malformed input: bounded output backpressure
   triggers fatal shutdown, exits 1 within the test's ten-second guard, and does
   not hang the UI on a pipe write. The renderer's own drain deadline is two seconds.

## Interactive native acceptance

The committed fixture was supplied through stdin and allowed to reach EOF. A normal
native window remained open and interactive. The Home-style text grid was visibly
rendered using fixed cells and Menlo on macOS. The transparent 32×48 PNG appeared
at zero-based cell `(8,4)`, with magenta soles and a yellow center marking its feet.

Visual inspection matched the tested geometry: grid-relative image top-left
`(120,72)`, feet `(136,120)`, and 16×24 cells. The grid's origin is the native content
origin below the titlebar. There is no padding or sprite-dependent change to cell
size. Screenshots were inspected in the renderer task; no production artwork was used.

The native UI inspector cannot select a bare command-line executable by app identity.
For the visual/key/close checks, the exact compiled binary was copied into a temporary
`target/Ichiloto Renderer.app` bundle with a minimal Info.plist; its executable was
still launched directly with the same fixture stdin and separately redirected
stdout/stderr. No alternate renderer code or test-only input path was involved.
The bundle is ignored build output, not a distribution artifact. The subprocess
smoke checks use the unbundled `target/debug/gpui-renderer` binary.

The final build received these actual native key presses, in order:

```text
Up Down Left Right w a s d Shift-w Shift-a Shift-s Shift-d
Return Space Escape q Shift-q
```

The final capture contained **exactly 19 valid NDJSON lines**:
`ready`, all 17 corresponding `key` events, then `close_requested`.
The complete capture is [s1-native-events.ndjson](s1-native-events.ndjson).
Uppercase W/A/S/D/Q were preserved. Escape and q/Q did not close the window or
trigger gameplay. The picture remained static. Stderr was **0 bytes**.

Clicking the actual native close button flushed `close_requested`, removed the
window, and exited **0**. The native application inventory confirmed it was no
longer running. This was repeated against the final build after the IPC cleanup.

## Implementation and handoff

Added or populated: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.gitignore`,
`src/main.rs`, `src/app.rs`, `src/protocol.rs`, `src/transport.rs`, `src/state.rs`,
`src/renderer.rs`, `src/assets.rs`, `src/input.rs`, `fixtures/home-frame.ndjson`,
`fixtures/test-sprite.png`, `scripts/fixture.py`, `scripts/native-smoke.py`, `README.md`,
and this validation record/capture. Removed the empty `src/scene.rs` placeholder:
the renderer has no scene concept. The pre-existing AGENTS.md, LICENSE, and local
settings were left as supplied.

Architecture: typed NDJSON → blocking stdin worker and asset preparation → bounded
async queue → GPUI presentation entity; native keyboard → key normalization →
nonblocking event queue → dedicated protocol writer. The UI never waits on stdin,
PNG decoding, stdout, or stderr. Full valid frames replace state atomically and
trigger `Context::notify`; no simulation or periodic render loop exists.

The exact public GPUI API inventory and protocol semantics are documented in
[README.md](../README.md#architecture-and-boundaries). Key rendering APIs are
`Application::new/run`, `App::open_window/spawn`, `Render`, positioned `div`/`img`
elements, `RenderImage::new`, `Context::notify`, `on_key_down`, and
`Window::on_window_should_close`. No extra platform crate was needed.

Discovered GPUI/environment limitations:

- Xcode was present but the Metal compiler component was missing. Installing
  Apple's official Metal Toolchain fixed the build; no GPUI fork or shader workaround.
- macOS `App::quit` delegates to Cocoa process termination instead of returning
  to `main`. The implementation now drains IPC asynchronously before quitting and
  selects a fatal exit status explicitly; native process tests verify both cases.
- GPUI's transitive `block 0.1.6` and `proc-macro-error2 2.0.1` dependencies report
  future-compatibility warnings under Rust 1.98.1. They do not fail the current build,
  strict project Clippy gate, or tests. No dependency source was patched.

S2 considerations, discussed with the Engine coordinator: PHP must supervise process
lifetime and consume ready/error/close events; retain camera-to-screen conversion,
bindings, frame timing, and gameplay. Unicode text currently uses scalar cells,
not Engine TerminalText's display-width/grapheme rules. Image reuse is per frame,
not a persistent cache. Backpressure is bounded and input is processed in arrival
order, without frame dropping; S2 should evaluate production cadence and caching.
Modifier chords, key-up events, and non-macOS native validation remain future work.

Protocol clarifications: short text rows blank-fill, empty sprite lists clear,
invalid frames preserve prior state, equal-layer order is stable, and EOF after
hello differs from shutdown. GPUI dimensions are logical pixels; display scaling
applies after grid layout to both text and sprites. These decisions match the
coordinator's zero-based PHP screen-coordinate and case-sensitive key contracts.
