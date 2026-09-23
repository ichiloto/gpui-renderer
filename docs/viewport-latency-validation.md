# Viewport fitting, resizing and input latency

Renderer-owned follow-up to S7-R baseline
`52951b45d04d70180d60a0507b28b1dcd3f972b9` on `develop`.
Status: implementation and native validation complete; Engine final review approved on2026-09-12.

## Scope and implementation

Only this renderer repository is changed. Engine, Console, Last Legend, and the
Engine-staged renderer installation are outside this change. Fixtures are silent,
contain no PHP/game process or audio, and run one native window at a time. The
previous fixture exits before the next starts. No repeat filtering/throttling,
protocol schema changes, frame validation changes, or gameplay behavior changes.

`window_layout.rs` uses the pinned GPUI 0.2.2 macOS implementation's actual window
coordinate semantics. `PlatformDisplay` exposes full bounds but no work area;
`MacDisplay::bounds` drops global display origins. The adapter reads
`NSScreen.mainScreen`/`screens`, `frame`, `visibleFrame`, and `NSScreenNumber`, matches
the ID to GPUI, and passes `display_id` explicitly. AppKit's class methods
`contentRectForFrameRect:styleMask:` and `frameRectForContentRect:styleMask:` use
the same titled/closable/resizable/minimizable style as this normal GPUI window.
No monitor, menu, Dock or titlebar dimension is hardcoded.

The fitted content and actual chrome are selected before centering the outer frame.
Content dimensions round inward and the top-left aligns to whole points. Native
inspection found that a fractional initial Y of48.5 made AppKit expand content by
one point; aligning it to48 removed that discrepancy. Centering may therefore have
a sub-point rounding difference. Tests cover display-relative conversion on primary,
negative-origin and nonzero-origin displays. Only the primary development display
was exercised physically. Host: macOS 26.6.2 (25G83), Apple Silicon; Rust/Cargo 1.98.1. Cocoa 0.26.0 / objc 0.2.7 direct macOS dependencies were already
in Cargo.lock; no registry/vendor/GPUI source was edited.

Non-macOS code remains cfg-separated, but no cross-target build was performed.
Until a usable-area adapter exists for another platform, native creation explicitly
fails there instead of presenting full display bounds as a verified work area.

`ViewportTransform::fit` is deterministic and independent of GPUI. It computes
`min(1, viewportWidth/logicalWidth, viewportHeight/logicalHeight)` and centers the
scaled surface. One clipped inner surface applies its centering offset; text cells,
font/line height, backgrounds, sprite origins, dimensions and bottom-center anchors
share its scale. A zero-sized viewport keeps the focus root with no painted surface.
GPUI refreshes the view when `viewport_size` changes. No stored frame or hello field
is mutated on resize; no resize event is emitted to PHP.

This separates logical surface size, initial native-size policy, current viewport,
and painting. Future preferred/fixed window sizes, scale settings or fullscreen can
extend those policies without changing protocol coordinates. Large-map scrolling
remains an Engine camera operation over the fixed logical viewport. User-facing
window preferences and a fixed native-window option are intentionally deferred.

## Native geometry and visual evidence

All dimensions below are AppKit points, not physical Retina pixels. The display is
1920×1200; `visibleFrame` is `(0,77,1920,1093)` in AppKit global bottom-left
coordinates. In display-relative top-left coordinates its usable bounds are
`(0,30)` through `(1920,1123)`. AppKit reports32 points of native chrome for this
window style, yielding1920×1061 available content.

| Case | Logical surface | Native content | Native outer frame (left,top,width,height) | Scale |
| --- | --- | --- | --- | --- |
| Oversized135×36 at20×40 | 2700×1440 | 1920×1024 | (0,48,1920,1056) | 0.711111 |
| Height-constrained135×36 at20×80 | 2700×2880 | 994×1061 | (463,30,994,1093) | 0.368148 |
| Last Legend135×36 at10×20, initial | 1350×720 | 1350×720 | (285,200,1350,752) | 1 |
| Shrunk | unchanged | 803×411 | (285,200,803,443) | 0.570833 |
| Enlarged beyond1× | unchanged | 1600×868 | (285,200,1600,900) | 1 |
| Approximately half | unchanged | 674×361 | (285,200,674,393) | 0.499259 |
| Final stable resize | unchanged | 1100×668 | (285,200,1100,700) | 0.814815 |

All four interactive fixtures’ initial outer bounds lie wholly inside that measured
work area. The complete cyan border marks all logical rows and columns. Sprite feet
remain attached to their authored cell; the text/sprite/UI order, default blank
occlusion and coloured blank stripes remain visible at small sizes. At1600×868,
the authored 1350×720 surface stays1× with offsets `(125,74)`.

Eight resize transitions were exercised in the same Last Legend process, including
five successive drags through1100×668,800×568,1100×668,674×361,1100×668. The accepted
frame remained visible, and a subsequent `second` fixture command replaced only
styles visibly. Without an intervening focus click, arrows/WASD/C/M/T/Tab/F5 emitted
their expected identities after resizing. No renderer restart, second hello,
protocol version change or protocol error occurred in those checks.

Evidence is in [evidence/viewport-latency](evidence/viewport-latency). Screenshots
are the native CUA capture bytes, renamed to `.jpg` to match their JPEG format.
The tool scales larger screenshots to its output limit; use diagnostic geometry,
not the screenshot pixel dimensions, for measured native sizes.

- [Height-constrained initial surface](evidence/viewport-latency/tall-initial.jpg)
- [V1 initial surface](evidence/viewport-latency/v1-initial.jpg)
- [V1 exact half scale](evidence/viewport-latency/v1-half.jpg)
- [Oversized initial surface](evidence/viewport-latency/oversized-initial.jpg)
- [Last Legend initial1×](evidence/viewport-latency/last-legend-initial.jpg)
- [Smaller viewport](evidence/viewport-latency/last-legend-small.jpg)
- [Approximately half scale](evidence/viewport-latency/last-legend-half.jpg)
- [Larger viewport, centered1×](evidence/viewport-latency/last-legend-large.jpg)
- [Style-only replacement after resizing](evidence/viewport-latency/last-legend-resized-style-replacement.jpg)

## Renderer input latency

Opt-in `ICHILOTO_GPUI_TRACE=1` observes the GPUI key-down callback, normalization,
writer submission, writer start, and successful complete stdout write plus flush.
Records have correlated IDs, process-local monotonic nanoseconds and actual GPUI
`is_held`. Stderr has a separate bounded worker; observation never performs stderr
I/O on GPUI's thread. Normal protocol stdout retains only its existing fields.

The callback timestamp does **not** measure OS-to-GPUI event delivery or dispatch
delay. These results also do not measure PHP consumption, frame receive/replacement,
GPU presentation, or end-to-end game latency. They concern a debug renderer with
diagnostics enabled and a regularly drained local stdout file.

| Controlled interval | IDs | Count | Median callback→flush | Maximum callback→flush |
| --- | --- | --- | --- | --- |
| Single Right | 1 | 1 | 2.183208ms | 2.183208ms |
| Single Enter | 2 | 1 | 0.075750ms | 0.075750ms |
| Rapid Right×10 | 3–12 | 10 | 0.611459ms | 5.356334ms |
| Keys after resizing | 13–25 | 13 | 0.119125ms | 1.477250ms |
| User-confirmed physical Right hold | 26–84 | 59 | 0.202708ms | 2.787542ms |

Scripted rapid taps arrived at 59.409 events/s over their first-to-last interval
(median spacing16.370ms, maximum31.012ms); writer completions measured 61.382/s over
their own interval. These short interval rates have different endpoints, not
different event counts. All 10 completed. The maximum pending queue snapshot was 1
before enqueue and 1 after dequeue; the maximum submission-through-flush outstanding
count was 2, including an in-flight write. This transient burst did not build a
sustained backlog. Single keys and the post-resize interval observed no pending
events in their queue snapshots. All scripted events had `is_held=false` and are
not evidence of OS repeat.

The user confirmed a physical Right hold and release via the Engine coordinator.
No automation input occurred during that interval. IDs26–84 are59 Right events over
5.229581 seconds. The first repeat gap is477.9205ms; IDs27–84 then arrive at11.9958/s
(median83.427ms, maximum86.376ms spacing), with11.9956 writer completions/s over
that interval. Callback→flush remains below2.788ms for the whole physical interval;
no pending queue events were observed, and peak outstanding is1. No further key
arrived betweenID84 and native close. Key-up is not instrumented, so no release
latency is claimed.

**Every physical-hold event still reports `is_held=false`.** Pinned GPUI's macOS
`window.rs` routes nonprinting keys through AppKit input-context command handling
(around1710); `do_command_by_selector` (around2320) reconstructs a `KeyDownEvent`
with `is_held: false`. `events.rs` initially reads `isARepeat`, but this command
path loses the marker. This is a GPUI metadata limitation, not evidence that the
user tapped. The logger preserves the field exactly; it never manufactures held
flags. The analyzer’s `--held` filter therefore returns no records for this physical
interval; select its confirmed IDs. No GPUI patch or repeat behavior change was made.

All84 Last Legend keys correlate through every stage to the exact stdout key
sequence, with no missing stage and zero dropped diagnostics in the closing summary.
The normal stdout file contains only ready,84 keys and close_requested, all protocol2.
The v1 fixture contains ready,Right,C,close_requested, all protocol1. Its resize is
exactly640×288 content for a1280×576 logical grid (scale0.5). Every interactive
fixture closed normally with exit0; the final process inventory found no renderer,
fixture driver or game process running.

Reproduce reports with `scripts/analyze-trace.php TRACE --events STDOUT`, optionally
`--ids FIRST:LAST` or `--held`. It correlates each stage by ID/timestamp and checks
stdout event identities against complete traces. Queue snapshots are separate
observations; they are not summed. Submission-through-flush lifetimes are reported
as a separate outstanding measure.

## Automated and lifecycle validation

- 52 Rust tests pass: the 41 S7-R regressions plus viewport/anchor/text/diagnostics
  coverage. Includes exact/smaller/constrained/wide/tall/zero viewports, centering,
  no upscale, bottom-center at1/.75/.5, styled glyph/opaque space bounds at1/.5,
  negative monitor origins, disabled traces, monotonic stage identity, and short
  writes/failed flushes without false `written` records or stdout schema changes.
  A blocked-stderr test also confirms observations are dropped without blocking the producer.
- Clippy `--all-targets --locked -- -D warnings` and `cargo build --locked` pass.
- Final format,52-test,Clippy and build checks pass (logs retained in evidence).
- All13 native smoke cases pass, including v1/v2 routing, style replacement,
  recovery, shutdown drain, broken stdout and unread stdout. Diagnostics are off
  by default for this suite; stdout retains the protocol schema.
- Four sequential interactive fixtures pass initial fitting, v1/v2 drawing, resize,
  focus/input, physical-hold observation and native close. The height-constrained
  outer frame exactly spans the measured1093-point usable height.
- Engine final review found no blocking correctness issue, independently reran
  all52 tests and diff checks, and verified every evidence hash. Approval covers
  macOS acceptance only, not Linux/WSLg readiness.

Existing upstream future-compatibility notices for `block 0.1.6` and
`proc-macro-error2 2.0.1` remain; no new renderer warnings were introduced.

## Engine handoff

Use the eventual landed renderer build without changing protocol coordinates.
Initial fitting and resize behavior are renderer presentation policy; keep the
Engine camera/logical grid authoritative. Current measurements show low
callback-to-stdout latency in the discrete and user-confirmed physical-hold intervals, but do not rule
out upstream native delivery delays or downstream PHP/presentation delays. The
separate Engine latency/stale-surface investigation remains open. No speculative
repeat behavior change was made.

Current `Instant` values have a process-local epoch and cannot be subtracted from
PHP `hrtime()`. The Engine coordinator verified its installed PHP build uses
`clock_gettime_nsec_np(CLOCK_UPTIME_RAW)`. A separately scoped stderr-only paired
clock anchor could align that absolute clock with renderer-relative timestamps,
with bracketed reads to quantify alignment uncertainty. No such anchor or frame
receive/replace/paint instrumentation is included in this change.

## Build provenance

Oversized and Last Legend visual/latency fixtures used the implementation build
SHA256 `4555b03940415090025819c1ea00993b3a33478dc0afc87625adf6afe4b36597`.
A subsequent change added only a `cfg(test)` blocked-stderr regression; runtime
implementation was unchanged. Rebuilding with that test source yields debug binary
SHA256 `b5de5b6dbbd94c463a61cde31064fdb1bf9438abe69d23d1bc50465776ca4798`.
The final v1/tall interactive fixtures and all13 smoke cases used this final build.
The renderer-owned app bundle now contains that final binary. The Engine-staged
installation was not changed. Raw results, reviewable measurements and file hashes
are recorded in `evidence/viewport-latency/validation-summary.json` and `SHA256SUMS`.

The callback-to-flush measurements do not show a material sustained delay in the
renderer output path tested here. A transient rapid-tap queue length of1 drained;
manual repeat never accumulated a queue. They cannot exonerate OS/GPUI dispatch or
PHP/frame presentation outside this boundary. The Engine's separate pump fix and
cross-process diagnostic work are separate changes and were not tested by these
standalone fixtures.

Utility links and reproduction commands now point to PHP equivalents. Recorded
native measurements above predate this utility migration; the migration did not
rerun native validation or change retained measurement data.
