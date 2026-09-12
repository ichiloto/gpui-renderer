# Clock anchors and frame lifecycle observations

Separate follow-up to reviewed viewport baseline
`b1a64256f22a34027e5b7328db27dd050f27b41f` on `develop`.
Status: implementation and native validation complete; Engine final review approved on2026-09-12.

## Scope

The Engine coordinator requested this renderer-owned measurement extension for the
S7-E latency/stale-surface investigation. It adds only opt-in stderr observations;
normal v1/v2 schemas, event order, queue capacities, frame replacement, paint order,
key-repeat behavior and gameplay ownership remain unchanged. No Engine, Console,
game, GPUI dependency source or Engine-installed renderer bundle was edited.

Changed runtime files are `diagnostics.rs`, `transport.rs`, `state.rs`, `app.rs` and
`renderer.rs`. A macOS-only direct `libc =0.2.189` dependency reuses the already locked
version. The remaining changes are tests, analyzer handling, documentation and
native evidence. There is no platform-adapter expansion or Linux/WSL acceptance.

## Clock boundary

`ICHILOTO_GPUI_TRACE=1` emits startup and shutdown `clock_anchor` records. Their
process-relative Instant values bracket a `clock_gettime_nsec_np(CLOCK_UPTIME_RAW)`
read. The function signature/availability were checked in the active Darwin SDK's
`usr/include/_time.h`; the numeric clock ID and ABI type come from libc, not a copied
constant. Non-macOS builds report an unavailable anchor instead of fabricating one.

The Engine coordinator verified the installed PHP8.5.10 preprocessor branch uses
`clock_gettime_nsec_np(CLOCK_UPTIME_RAW)` for `hrtime()`. A separate PHP CLI process
recorded `hrtime(true)` before and after renderer startup, and before and after its
native close. Both renderer host-clock samples lie inside the corresponding PHP
enclosures, confirming shared epoch/units for this host. The marker commands were:

```sh
php -r 'echo hrtime(true), PHP_EOL;'
```

The raw markers and anchors are preserved in
[evidence/frame-diagnostics](evidence/frame-diagnostics). The captured renderer
PID was83239. Its startup bracket spans20,750ns; its shutdown bracket spans833ns.
Their offset intervals intersect at
`[145552892120375,145552892121208]`ns over the approximately88-second capture.
That intersection provides a consistent833ns alignment interval for this capture;
it is not a universal synchronization guarantee.

For an anchor, compute signed/integer `offsetLow = host_ns - elapsed_after_ns` and
`offsetHigh = host_ns - elapsed_before_ns`. A relative event time `t` maps to
`[t+offsetLow,t+offsetHigh]` on the same host clock. Check the other emitter's actual
clock, bracket widths and start/end consistency before using this mapping. Never
subtract unrelated process-local Instant epochs. No periodic anchor timer or
cross-machine synchronization was added.

## Frame observations

Frames carry an optional internal trace context with renderer-local receive
sequence, incoming protocol version and source frame number. Sequence is allocated
only when enabled and after parsing identifies a frame, while the `received`
timestamp is sampled immediately after reading its complete bounded line and before
parsing. This excludes partial-line arrival time and does not acknowledge a frame.

`accepted` follows successful session/version/frame validation and PNG preparation,
before the bounded update channel. `replaced` follows the actual app handler's state
replacement, before notify. `render_callback` is emitted on callback entry for the
current prepared snapshot. The context changes no logical coordinates or stored
presentation values. Disabled tracing leaves it absent.

Callbacks may repeat for one sequence or skip an intermediate replaced snapshot
when GPUI coalesces work. Callback entry is not completed layout, GPU submission,
present completion or visible-pixel proof. Screenshots provide separate visual
evidence at sampled points. A missing acceptance/callback must be interpreted with
ordinary errors, dropped diagnostics and capture/shutdown boundaries.

Each diagnostic is serialized on the background worker and submitted as a complete
line so ordinary stderr errors cannot split serde's field-by-field writes. Existing
bounded, nonblocking record submission and dropped-record reporting are retained.
The key analyzer accepts ordinary stderr messages separately but rejects a damaged
diagnostic line; stdout validation remains strict.

## Native validation

One silent native v2 fixture ran on macOS26.6.2 (25G83), Apple Silicon, using
Rust/Cargo1.98.1 and GPUI0.2.2. Its binary SHA256 is
`4ee59a0e0f34ec776af97120631ea4aea3c09de4d8a3786d4a8c664d4a824a58`.
No other native renderer/game window was open.

| Receive sequence | Source frame | Result |
| --- | --- | --- |
| 1 | 1 | Received, accepted, replaced, two render callbacks; first image visible |
| 2 | 2 | All stages; unchanged glyphs with changed colours visible |
| 3 | 1 | Missing PNG: received only; ordinary error and prior image preserved |
| 4 | 1 | Repeated/decreasing source number accepted as a distinct sequence; first colours restored |
| 5 | 3 | Empty snapshot accepted/replaced/callback; presentation visibly cleared |

See [first](evidence/frame-diagnostics/first.jpg),
[second](evidence/frame-diagnostics/second.jpg),
[rejected frame retaining second](evidence/frame-diagnostics/rejected-keeps-second.jpg),
[repeated frame one](evidence/frame-diagnostics/repeated-frame-one.jpg), and
[cleared](evidence/frame-diagnostics/cleared.jpg). Images are unmodified native CUA
JPEG captures. The fixture commands were `second`, `invalid`, `first`, `clear`,
followed by normal native close. One native Right key verified the existing key
trace/protocol route. No scripted or physical repeat test was repeated in this
follow-up; the baseline's repeat semantics are unchanged.

| Sequence | Received→accepted | Accepted→replaced | Replaced→first callback |
| --- | --- | --- | --- |
| 1 (startup) | 1.967459ms | 1187.809291ms | 61.029417ms |
| 2 | 7.188750ms | 0.115584ms | 15.884041ms |
| 4 | 12.484292ms | 2.431167ms | 13.541833ms |
| 5 | 0.380916ms | 0.109625ms | 11.251250ms |

These sparse fixture timings validate the measurement boundaries, not game
performance or a steady-state latency distribution. The startup interval includes
native application/window initialization. PNG preparation is included before
acceptance. The trace does not establish when the resulting GPU image became
visible; the later screenshots confirm the selected states independently.

The fixture stdout contains exactly `ready`, Right `key`, one expected `error`, and
`close_requested`, all protocol2 with no diagnostic fields. The closing diagnostic
summary reports zero dropped records. Native close exits0 and the fixture driver
exits; no renderer process is left running after validation.

## Tests and handoff

- 55 Rust tests pass, including all52 baseline tests. New coverage verifies bracketed
  host-clock observations, duplicate/decreasing v1/v2 frame numbers with distinct
  sequences, rejected assets producing no acceptance, disabled observation context,
  and complete diagnostic-line writes. Non-macOS unavailable-anchor coverage is
  cfg-selected but was not run on a non-macOS target.
- Format, Clippy with warnings denied, and build pass. The existing upstream
  future-compatibility notices remain unchanged.
- The 13-case native smoke suite passes with diagnostics disabled to verify unchanged
  normal startup, shutdown, recovery, v1/v2 routing and output backpressure behavior.

Raw traces, stdout, PHP markers, screenshots, key analysis, frame/clock validation
summary and check logs are retained with `SHA256SUMS`. The Engine should correlate
its send/drain and consumption timestamps only through validated clock anchors and
matching source-frame identity, accounting for renderer-local sequence. Use received,
accepted, replaced and callback boundaries to locate a stale-surface interval, then
verify actual visibility separately. This work does not itself validate the Engine's
new pump behavior or close its real-game end-to-end acceptance gates.

Engine independently reviewed the final code and native state screenshots, reran
all55 tests, and verified diff cleanliness and every evidence hash before approving
this separate develop commit. Approval does not imply Linux/WSLg or end-to-end
Last Legend acceptance.
