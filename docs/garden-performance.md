# Garden of Roads output investigation

2026-09-12. Removing the physical terminal mirror reduces the matched private
Garden workload from 9.947 seconds to 6.041 seconds (39.3% less elapsed time).
Median field recomposition drops from 178.000 ms to 78.233 ms. GPUI's measured
CPU draw cost is essentially unchanged. Engine also fixed accumulating style
history in shared text processing. A subsequent foreground-confirmed run with
both fixes completed the timed route in 3.280 seconds, with median field
recomposition of 25.891 ms and snapshot creation of 7.525 ms. The initial Garden
visual check and automatic closure are verified. These results measure this
workload, not visible FPS or a complete Chapter 3 playthrough.

Engine owns the output-selection and text-processing changes. This renderer task
reviews the boundary and analyzes retained traces. The existing S8/readability
renderer candidate stays unchanged (`20a38d04d44f7eb0dd3469dadabdc2b7cf567487bac14db34bd73608bfa6a584`).

## Matched workload

Engine prepared two private runtimes from a Garden of Roads checkpoint. Both use
the same 135 × 36 logical grid, 10 × 20 cells, release renderer, muted settings,
random seed, 12 idle iterations and 36 prescribed camera positions. The camera
is detached from the player, moved through the same positions, and the ordinary
field compositor is called. This measures a repeatable scene workload; it does
not reproduce physical held-key input or the complete Chapter 3 journey.

The comparison-only subclass enables terminal output for the mirror control.
Production GPUI selects buffer-only output. Canonical scene composition, styled
layers, sprite collection and graphical frame export remain active. Terminal
mode retains its physical rendering. Output selection skips terminal-only dirty
span calculation, ANSI payload construction, writes, and terminal screen controls.
It is not a map-specific rendering rule or an image filter.

Both runs completed 48 iterations and submitted 36 graphical snapshots. All 36
snapshots reached receipt, acceptance, replacement and a complete CPU draw cycle
in each run. There were no dropped diagnostic records, malformed/truncated
renderer diagnostic lines, or ordinary renderer stderr messages. Both processes
exited with code 0; terminal settings matched before/after. Engine also reported
empty game error logs.

CUA app selection timed out during the mirror run. No screenshot or native
foreground/occlusion confirmation was retained for this comparison. Consequently
callbacks and `paint_end` are CPU observations, not proof of visible presentation,
GPU completion, visible FPS or input latency. Sequential single runs also contain
ordinary system-load variation; the percentage is specific to this workload.

## Measurements

Stage values are medians in milliseconds. Field recomposition has 36 observations;
snapshot/export has 48. Native stages have 36. Total elapsed time includes the
48-iteration workload's configured pacing; it is not a pure CPU sum.

| Boundary | Terminal mirror | Buffer-only |
| --- | ---: | ---: |
| Total workload, seconds | 9.947318 | 6.040714 |
| Field recomposition | 177.999854 | 78.233063 |
| Shared presentation snapshot | 25.627021 | 25.098500 |
| GPUI receipt → accepted (parse/validate/prepare) | 0.506980 | 0.505042 |
| GPUI accepted → replaced | 0.023292 | 0.025646 |
| GPUI replaced → first render callback | 8.948292 | 8.413646 |
| GPUI render callback → CPU paint end | 33.239604 | 33.426105 |
| GPUI CPU paint method alone | 21.804500 | 22.130521 |

The unchanged native preparation/draw times and substantial reduction in PHP
recomposition support removing duplicate terminal work. The remaining roughly
25 ms snapshot and 78 ms recomposition costs motivated the shared style fix below.
No renderer queue, scheduling, image-cache or rendering-algorithm rewrite was
made for this comparison.

The retained geometry also records the actual font measurement absent from the
earlier readability check: Menlo advance 0.60205078125 em, line metric
0.6923828125 em, logical font size 15.779399871826172 px. Both viewports were
1350 × 720 at scale 1.0. This is new evidence from these Garden runs, not a
retroactive measurement of the previous readability run.

## Shared style normalization follow-up

`TerminalText::visibleSymbols` previously retained every selective SGR reset and
colour change in each subsequent cell's prefix. This creates quadratic growth
for common alternating styles: 200 repetitions of a green `X`, foreground reset,
and space expanded 2,400 input bytes into 403,000 cell bytes. Engine's new
`SgrStyleState` retains the active attributes instead. The same case now uses
2,200 cell bytes. This benefits shared styled text and UI; it does not alter map
art, image tiles or sprite pixels.

The normalizer preserves independent foreground/background and terminal
attributes, selective and full resets, and grouped indexed/RGB colour components
(including zero values). Unknown or malformed sequences conservatively retain
their replay order until a full reset. Renderer-side read-only review found no
blocking issue. An additional deterministic integration check compared colour
extraction before/after normalization for 5,000 prefix cases, with no mismatch.

The third private arm used the same workload and recorded all 36 frames through
replacement, but **zero game-frame render callbacks or CPU paint observations**.
Its Engine field recomposition median was 31.298417 ms and presentation snapshot
median 7.924417 ms; its 3.701893-second total is **not a matched native-rendering
comparison** with either prior arm. In particular, different native CPU/GPU
activity may affect concurrent Engine timings. Do not derive an overall speedup
or visible FPS from that total.

All diagnostic chunks were complete with no reported drops/errors, process exit
was 0, and terminal settings matched before/after. After the run, Engine's
`cua.getState` reported that the Mac was locked and automatic unlock failed.
That is an after-run observation; it does not establish when the machine locked
or prove why the earlier callbacks were absent. A foreground visual check could
not be completed in that state. The trace is retained under
[`normalized-unconfirmed-draw`](evidence/garden-output/normalized-unconfirmed-draw/summary.json)
instead of combining it with the native CPU draw distributions.

## Foreground-confirmed follow-up

After the user unlocked the Mac, Engine repeated the normalized arm with a
startup-only gate. The [gated driver](evidence/garden-output/normalized-foreground/gated-driver.php)
first presents one initial field frame and waits up to 45 seconds for a private
`go` sentinel. Engine obtained the active installed app through CUA, confirmed
the focused Garden window, and captured readable labels, blue water, the
coordinate/heading HUD and the PNG player. It then released the unchanged
12-idle/36-pan workload. The gate wait is outside the measured interval.

The full session contains 49 presentation snapshots and 36 submitted graphical
frames. Excluding the pre-gate presentation leaves **48 measured snapshots and
35 submitted workload frames**. All 35 workload frames have complete receipt,
acceptance, replacement and CPU draw cycles; there are no diagnostic drops,
ordinary errors or truncated chunks. The initial stable frame is already cached,
so the timed work does not submit it again. This startup warmup differs from the
earlier controls and must be kept visible when comparing totals.

| Measured foreground workload | Result |
| --- | ---: |
| 48-iteration elapsed time | 3.279921 s |
| Field recomposition, 36 samples | 25.890667 ms median |
| Presentation snapshot, 48 samples | 7.524750 ms median |
| GPUI receipt → accepted, 35 frames | 0.493042 ms median |
| GPUI accepted → replaced, 35 frames | 0.023000 ms median |
| GPUI callback → CPU paint end, 35 draws | 34.686666 ms median |

The initial foreground screenshot establishes that the scene was visible and
readable before the timed workload. The later screenshot request timed out as
automatic closure completed; it is not a final-position screenshot. Process exit
was 0, game error/stderr logs were empty, and terminal settings matched exactly.
Engine subsequently confirmed through CUA inventory that the renderer had
exited. No test window remains waiting for input.

The earlier `normalized-unconfirmed-draw` evidence stays separate. It has not
been relabeled or merged with this successful foreground run. The scoped
summary excludes startup by `probe.start` and matches unique Engine queued frame
numbers to their native records, without subtracting unrelated clock epochs.
See also [Engine's validation report](../../../engine/docs/rendering/garden-output-validation.md).

## Evidence and reproduction

- [Mirror summary](evidence/garden-output/mirror/summary.json), Engine PID 96843.
- [Buffer-only summary](evidence/garden-output/buffer-only/summary.json), Engine PID 97013.
- [Style-normalized CPU-only summary](evidence/garden-output/normalized-unconfirmed-draw/summary.json), Engine PID 4807.
- [Foreground whole-session summary](evidence/garden-output/normalized-foreground/summary.json),
  [foreground workload summary](evidence/garden-output/normalized-foreground/workload-summary.json), Engine PID 11176.
- [Retained workload](evidence/garden-output/driver.php).
- [Analysis script](evidence/garden-output/analyze.php), [source receipts](evidence/garden-output/receipts.json).

The full retained Engine traces contain the renderer stderr chunks, rather than
assuming that stderr chunk boundaries match diagnostic lines. The analyzer
reassembles them, correlates frames by renderer-local sequence, pairs each draw
occurrence separately, checks host-clock anchor consistency, and reports drops
and incomplete records. It makes no cross-process duration claims from unmatched
clock epochs.

```sh
php docs/evidence/garden-output/analyze.php \
  docs/evidence/garden-output/mirror/engine.ndjson \
  --pid 96843 --out /tmp/ichiloto-garden-mirror-review
php docs/evidence/garden-output/analyze.php \
  docs/evidence/garden-output/buffer-only/engine.ndjson \
  --pid 97013 --out /tmp/ichiloto-garden-buffer-review
php docs/evidence/garden-output/analyze.php \
  docs/evidence/garden-output/normalized-unconfirmed-draw/engine.ndjson \
  --pid 4807 --out /tmp/ichiloto-garden-style-review
php docs/evidence/garden-output/analyze.php \
  docs/evidence/garden-output/normalized-foreground/engine.ndjson \
  --pid 11176 --after-probe-start --out /tmp/ichiloto-garden-foreground-review
```

Engine's output-selection revision passed 1,324 tests with one skipped and
4,924 assertions. The final output-selection + style-normalization revision
passed **1,337 tests, one skipped, 4,954 assertions**, plus PHPStan with no errors.
A raw combined-SGR-byte assertion in a save-window test was updated to compare
the preserved colour meaning; the final suite above includes that correction.
Console's `composer test` also passes, with two existing opt-in integration skips.
The focused S6 graphical-player and Garden Slice A integrations passed all
15 tests (270 assertions), including normal and skipped arrival cinematics.
The broad game suite was deliberately stopped during its unrelated battle
simulation baselines; it is not reported as a completed full-suite pass.
Engine verified a terminal Garden run independently. Renderer source is unchanged
from the previously validated 68-test S8/readability candidate. The new native
visual evidence is the initial foreground Garden capture described above;
physical held-key input and an entire chapter playthrough are not claimed.

Utility links and reproduction commands now point to PHP equivalents. Recorded
native measurements above predate this utility migration; the migration did not
rerun native validation or change retained measurement data.
