# Graphical glyph effects validation

15 September 2026 validation of the implementation subsequently committed as
`b44b08a8393ab74c28a0ff5c947274c5cbfca4f1`, including clipping/opacity and portable
window handling. Current installation and workflow status are maintained in the
[README](../README.md#availability-and-installation). The earlier G1 executable
was replaced by this accepted build in the normal Engine installation on
16 September; the checkpoints below retain their original evidence.

## Scope and implementation

Engine relayed Andrew's explicit approval of exact direct `cosmic-text =0.14.2`
and `swash =0.2.10`. GPUI remains `=0.2.2`; Cargo.lock changed only to record those
two dependencies on the root package, with no resolved package version changes.
Fonts are discovered system monospace; no font download or bundled font was added.

The frozen `canvas_glyph_effects` contract extends local-grid canvas text. Engine
owns grid dimensions, scalar positions, literal strings/colors, derived effect
padding, placement and timing. Rust shapes scalar glyphs with COSMIC Text and uses
the same resolved font/contour for foreground and real Swash path strokes. A finite
Gaussian operates only on glyph coverage. The result is transparent BGRA; no
number atlas, text plate, offset-painted text approximation or private GPUI API
is used. A missing scalable glyph rejects the complete candidate.

Raster keys are scoped to the immutable session font catalog and include local
grid/runs/effects and device density. Position, layer identity/order, clip and
layer opacity do not change the glyph raster. Font discovery is retained; loaded
shaping/font/raster contexts are
released after each cache-miss batch, avoiding a second unbounded glyph cache.
Host font appearance can vary. The local-grid contract does not claim exact browser
font-size/kerning equivalence.

The display-raster pool is shared with terrain at its existing 64 MiB/32768 image
limits. Candidate admission counts all live registered rasters (including images
held by old snapshots or retirement queues), new output and peak mask/blur scratch.
Weak lifetime tracking keeps these bytes counted after LRU eviction without
retaining the pixels itself. The previous complete generation remains visible on
preparation failure; failed partially prepared candidates are discarded. Resize
preparation has the same gate and retains the previous generation if it fails.
Decoded PNGs and source-region caches retain their separate existing limits;
this is not a claim of a 64 MiB whole-process footprint or a GPU allocation cap.

## Automated evidence

- 106 optimized tests passed, including the original 139 G1 and 75 clip/opacity
  wire cases and the independent 90 glyph-effect wire cases.
- Glyph corpus manifest SHA256:
  `4d37e341ced397fba4fb37d180ab3ae6804d59722d612c9cf1e4294fa266e524`.
- Real system-font raster tests check transparency, contour pixels, blur falloff,
  guard pixels, fractional density and 100 motion/fade cache hits.
- A held 32 MiB tile raster continues to prevent a 40 MiB glyph candidate plus
  scratch after both LRU eviction and GPU-retirement release. Admission succeeds
  only after the final snapshot reference is released.
- A second-layer font failure after the first candidate layer is rasterized
  retains the old complete image and leaves no staged raster allocation live.
- Release build, Clippy across host target kinds with warnings denied, formatting
  and whitespace checks passed on macOS/Apple Silicon. Existing upstream
  future-compatibility notices remain for `block` and `proc-macro-error2`.

## Game-sized admission evidence

The separate external-fixture test passed all 32 cases: ordinary and real-action
feedback frames emitted by Game's actual `BattleScene`/`BattleScreen`, its explicit
six-recipient/six-result-line presentation stress case, and a separately labeled
equal-width numeric-variation derivative. Each was checked cold and with the prior
generation retained at densities 1, 1.5, 2 and 3 device pixels per logical pixel.
Real asset loading, capability negotiation and frame preparation passed before
glyph preparation. The input captures are unchanged; both stress cases are
presentation tests, not authored encounters or gameplay outcomes.

The numeric variation prevents repeated result numbers from hiding cache misses.
At density 3 it requires 25 distinct rasters and 46,356,960 bounded convolution
sample operations, below the 67,108,864 resource-work ceiling. Its peak admission
reservation is 5,088,314 bytes including the held density-2 generation and scratch.
The original stress case uses 13 distinct rasters, 22,817,632 operations and
2,435,722 reserved bytes. This exercises the existing work ceiling against actual
Game geometry without changing wire/run/layer limits. It is a renderer resource
check, not an additional negotiated protocol limit or a frame-time guarantee.

In this optimized host run, numeric-variation preparation at density 3 took
58.48 ms cold and 50.61 ms on scale change. Cached repetition took 0.095 ms with
zero raster builds. First process font discovery took 208.99 ms for the ordinary
frame; later cold cases benefit from system font/file caches. These are CPU glyph
preparation observations, not GPU presentation measurements or gameplay FPS.
The [full measurements](evidence/glyph-effects/headless/game-frames.json) and
[receipt](evidence/glyph-effects/headless/receipt.json) retain input, source and
candidate hashes. Normal tests pass 106 with this external test ignored; it was
then explicitly run and passed independently:

```sh
ICHILOTO_GAME_FRAME_DIR=/path/to/game/artifacts \
ICHILOTO_GLYPH_REPORT=/path/to/report.json \
cargo test --release --locked emitted_game_frames_fit_cold_and_retained_generation_budgets -- --ignored
```

The directory must contain `hello.json` with an existing real asset root and the
three `battle-{ordinary,feedback,six-recipient-stress}.frame.json` captures.

Temporary candidate bundles and generated build caches were removed after
canonical installation. Historical paths in the hashed receipts identify the
test inputs and are not active launch instructions. Reuse the accepted installed
executable identified in the README; do not recreate the retired runtime copies.

## Focused native validation

After Engine relayed Andrew's permission to proceed and the desktop was confirmed
unlocked, the candidate passed the silent four-frame native scenario on macOS.
The first bounded run exited successfully but app-path inspection timed out;
it provided no visual proof. After confirming closure, one further bounded run
selected the running application's identifier from CUA inventory and completed
visual inspection. Both runs opened one window at a time and exited with code 0.

The successful inspection verified [glyph contours and shadows](evidence/glyph-effects/native/01-effects.png),
the [smaller centered view](evidence/glyph-effects/native/02-restored.png),
[rise/fade/clipping after maximization](evidence/glyph-effects/native/03-maximized.png),
[plain text after effect removal](evidence/glyph-effects/native/05-plain.png), and
[complete clearing](evidence/glyph-effects/native/06-cleared.png). The 640x360
canvas fit the 320x320 restored content area at scale 0.5 with offsets 0/70 and
the 1920x1062 maximized content area at scale 1 with offsets 640/351. Every logged
viewport retained centered fit geometry, including intermediate resize sizes.

The scenario negotiated all three canvas capabilities, painted all four frames,
reported raster builds 1/0/0/0 at frame admission and zero dropped diagnostics.
Resize-triggered raster preparation is separate from those frame-admission counts.
The process exited with code 0 and CUA's subsequent app inventory confirmed the
owned test window was closed. The [native receipt and evidence hashes](evidence/glyph-effects/native/validation.json)
identify the tested candidate/source and the G1 executable preserved during
that checkpoint, before the later canonical installation.

The reusable silent scenario is `scripts/native-tile-smoke.php --glyphs --binary
<installed executable> --evidence-dir <directory>`. It checks negotiation, cached
rise/fade/clipping, effects omission and whole-canvas clearing, then closes its
owned process. With `--glyphs`, `--observe-seconds 45` holds each of the four frames
for inspection before bounded automatic shutdown.

## Isolated Game acceptance

The Game owner subsequently completed the bounded ordinary native battle on
macOS with the same executable and complete bundle, installed only into its
isolated Engine runtime. The legitimate File1 encounter used normal input and
gameplay RNG without stat, save or outcome manipulation. Panels/gauges, target
switching from rat to bat, grouped outlined rising `CRITICAL / 40 / KO`, victory
and field return passed. The Game process exited with code 0 and its native
window was confirmed closed. This is separate from the synthetic renderer check.

The final Game-owner report had SHA256
`fc7c65027bd8ab3a3dddeb001f0ce1b43cc237cb48cdfb9d6ccf871b401b970c`.
Game evidence and configuration were retained in the workspace archive
`archives/last-legend-spike-20260916.zip` before the temporary runtimes were removed.
Their former launch paths are obsolete; current playtesting uses the normal Game
checkout. The Game source, legitimate save and canonical installation remained
unchanged during this acceptance checkpoint. Automated terminal fallback/parity
checks passed 2 tests/5 assertions;
this was not an interactive terminal playtest. No further tests are pending for
this bounded acceptance.

## Validation limits

This checkpoint did not cover untimed status-effect popup callers, exhaustive
battle outcomes or the later Engine/Game cursor corrections. Current cursor
policy is in the [README](../README.md#cursor-presentation). Linux/WSLg and native
Windows builds/execution have not been validated. Local integration and canonical
installation do not constitute publication or cross-platform acceptance.

Utility links and reproduction commands now point to PHP equivalents. Recorded
native measurements above predate this utility migration; the migration did not
rerun native validation or change retained measurement data.
