# Ordinary Last Legend burst investigation

Status, 2026-09-12: renderer investigation and automated native checks complete;
Engine reviewed and approved the renderer change. The measured delay is foreground draw
work in an **unoptimized build**. The same diagnostic source compiled in release
mode reduces the matched Home handoff interval to 0.015–8.807ms and CPU paint to
7.897–9.926ms. No scheduling, queue-capacity, image-cache or rendering-algorithm
change was needed. The implementation adds opt-in diagnostics and build guidance.

This renderer task did not edit Engine, Console, game or dependency source.
Engine staged the reviewed debug/release executables at its ignored installation
boundary after user approval. S7-E stays uncommitted; S8 has not begun. Physical
hold/release, eligible T skit, battle and Engine's broader final acceptance remain
open. Renderer-local success does not close those gates.

## Baseline and launch

Renderer `develop` was clean and its local and live GitHub tips both equalled
`e7f550d221a412fc34f46b92293653db7d161c20`. The existing installed executable
matched SHA256 `4ee59a0e0f34ec776af97120631ea4aea3c09de4d8a3786d4a8c664d4a824a58`.
One ordinary Console launch ran on Apple Silicon, macOS 26.6.2 (25G83), with
GPUI 0.2.2, Rust/Cargo 1.98.1 and PHP 8.5.10. Console PID 7629 launched game
PHP 7644 and GPUI 7655. Filter by **game PID 7644** when reading Engine traces.

A fresh private runtime copied the previous private settings and saves, retaining
read-only assets/vendor symlinks. Its configuration was verified as master volume
0, music false and SFX false. The original runtime/evidence were preserved.
The ordinary entry point, from `/tmp/ichiloto-renderer-burst-20260912/runtime`, was:

```sh
stty cols 135 rows 36
ICHILOTO_ENGINE_TRACE=1 ICHILOTO_GPUI_TRACE=1 php /Users/andrewmasiye/Development/php/games/engines/ichiloto/2.0/console/bin/ichiloto play --directory=/tmp/ichiloto-renderer-burst-20260912/runtime --no-tmux --renderer=gpui
```

The title bar was clicked to focus the single native Last Legend window. New Game
and the opening dialogue were completed normally. All three diagnostic bursts used
exactly **Right, Left, Right, Left**, starting at free Home `(8,4)` and alternating
with `(9,4)`. Every consumed key had `event_owns_input=false`. The field's NPCs
continue their ordinary autonomous updates; these are not frozen replay frames.
No user input was requested during these automated runs.

## Reproduction

One ordinary launch contained two sampled bursts and one unsampled control.
All timings below are milliseconds. Callback means entry, not visible pixels.
A dash means that snapshot was replaced without an individual callback.

| Burst | Native input | Frame | Native→receipt | Receipt→accepted | Accepted→replaced | Replaced→callback |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| A, sampled | 6 Right | 140 | 29.208 | 1.081 | 5.001 | 6.850 |
| A | 7 Left | 141 | 37.704 | 3.681 | 153.444 | 0.062 |
| A | 8 Right | 142 | 30.425 | 2.721 | 0.096 | 8.126 |
| A | 9 Left | 143 | 42.996 | 1.231 | 124.448 | 0.066 |
| B, sampled | 10 Right | 162 | 26.131 | 1.444 | 0.066 | 3.825 |
| B | 11 Left | 163 | 39.825 | 0.973 | 155.486 | — |
| B | 12 Right | 164 | 67.980 | 2.525 | 125.372 | 11.613 |
| B | 13 Left | 165 | 25.908 | 0.803 | 136.763 | — |
| C, unsampled | 14 Right | 179 | 27.234 | 2.673 | 0.109 | 6.518 |
| C | 15 Left | 180 | 33.617 | 0.799 | 132.761 | 8.109 |
| C | 16 Right | 181 | 38.967 | 0.852 | 113.336 | — |
| C | 17 Left | 182 | 65.610 | 1.963 | 84.783 | 0.032 |

All twelve movement inputs were consumed in order and produced the expected
alternating coordinates. After each burst the visible position was `(8,4)`,
facing west. Coalescing does not by itself mean final state was lost: subsequent
complete field snapshots also contain the latest Player state. The independent
post-call screenshots verify the sampled final states, not reaction latency.
The unsampled control reproduces the long interval, so the sampling tool alone
does not explain the symptom.

[Summary](evidence/burst-investigation/baseline/summary.json) contains each input,
frame identity and exact received/accepted/replaced/callback host timestamp.
The complete extracted [renderer trace](evidence/burst-investigation/baseline/renderer.ndjson)
and [selected Engine records](evidence/burst-investigation/baseline/engine-selected.ndjson)
retain the source observations. Both start and shutdown anchors are present; their
offset intersection is `[150867832382958,150867832383583]`ns (625ns wide).
No diagnostic records were dropped, no stderr error occurred, and all 186 frames
were received, accepted and replaced. All 17 emitted keys match PHP parsing,
including the five startup/dialogue Return keys.

## Native stack collection and limits

The read-only sampler was armed before each burst. CUA wrote a request marker,
waited for the sampler's collecting banner, then sent the exact sequence. Burst B
used an intentional 1.5-second experimental offset after that banner to place the
burst later in collection. This offset is not added between taps.

```sh
/usr/bin/sample 7655 3 1 -file /tmp/ichiloto-renderer-burst-20260912/evidence/home-a-sample.txt
/usr/bin/sample 7655 5 1 -file /tmp/ichiloto-renderer-burst-20260912/evidence/home-b-sample.txt
```

[sample-window.py](evidence/burst-investigation/baseline/sample-window.py) records
command begin/end and collection banners with `CLOCK_UPTIME_RAW` host timestamps
and wall-clock markers. CUA records `Date.now()` immediately before/after every
`pressKey` call. Those millisecond wall brackets describe tool requests, with
wall-clock alignment limits; they are not the renderer's native timestamps or
proof of precise OS injection time. Frame correlation uses the validated native
host-clock anchors instead.

Sample A's command envelope is `[151103933968500,151113443574250]`ns;
its collecting banner is at 151104165949541. Sample B's command envelope is
`[151225220179250,151236481307541]`ns, with collecting/completed banners at
151225462049708 and 151234851320791. The bursts' native/frame timestamps fall
within those envelopes. **These are not exact per-stack sample timestamps.**
The tool requests nominal 3/5-second collections but also performs setup,
suspension/collection overhead and symbol processing. Its output condenses stacks
into a call tree without individual timestamps. In particular, the mostly idle
sample A must not be attributed to frame 141's particular 153ms interval.

| Report | Main thread | Protocol reader |
| --- | --- | --- |
| [A](evidence/burst-investigation/baseline/home-a-sample.txt), 2018 observations | 2009 in AppKit/CFRunLoop `mach_msg2_trap`; a few event-loop/dispatch maintenance stacks | all 2018 in `read_protocol_observed` → `read_until` → stdin `read` |
| [B](evidence/burst-investigation/baseline/home-b-sample.txt), 3694 observations | 3551 in the same event-loop wait; 135 at one `Window::draw` site plus 2 at another, including layout request, element construction and text/div paint paths | 3693 in stdin `read`, one parsing observation; no `send_blocking` stack |

The main-thread paint/layout observations in B establish that those paths ran
during the collection. Their aggregate counts neither measure one frame's CPU
cost nor establish what occupied each delayed interval. Writer/diagnostic threads
normally wait for their respective queues; those parking/condition-variable
stacks are not the protocol reader waiting for handoff capacity. The native event
thread waits in its normal AppKit Mach port path. No long blocking Metal/image
upload path is established by these reports.

PNG decoding is absent from sampled stacks. It is also completed before the
existing `accepted` marker for each relevant prepared frame, so it cannot explain
those same frames' later handoff interval. This rules it out as that stage's cause
in the sampled case, not all image-related CPU/GPU work or future scenes. The
viewport transform is part of element construction and has no material sampled
hotspot; its isolated cost is not measured by this baseline. Responsive resizing
has not been removed.

## Channel and automation conclusions

The reader uses one synchronous sender and a bounded two-update channel. Baseline
`accepted` is before `send_blocking`; `replaced` follows the foreground handler's
snapshot assignment. Their difference includes submission/backpressure,
foreground scheduling and replacement, as well as time the foreground thread may
spend finishing a previous render. Close/error/hello also share this channel.
The baseline has no enqueue-complete or dequeue observation, so it does not
quantify exact channel depth or submission duration. B's paired replacements
support queued updates draining together, while its reader stacks do not show a
capacity wait. Neither fact proves a channel defect.

CUA request brackets and native arrival gaps differ. For example, A's third native
callback follows its second by 342.983ms; B's fourth follows its third by 194.886ms.
The baseline cannot separate CUA delivery from foreground event handling within
those gaps. Delays *after* frame acceptance remain real measured renderer-path
intervals; explaining them requires the narrower observations below. No physical
four-tap comparison or held-key gameplay acceptance is claimed.

**Baseline-only conclusion:** reproducible post-acceptance delay; these initial
observations do not establish the responsible foreground/channel substage.
They provide insufficient causal evidence for a scheduling, capacity, image-cache
or repeat change. The subsequent diagnostic runs below locate the delay.

## Diagnostic implementation and initial checks

Candidate SHA256:
`58cfd712ef7123563afe9757f0bed09d18187576404e46f65029c0939b67f883`.
The initial candidate was built at `target/debug/gpui-renderer`. Engine reviewed
the full source, including `src/render_trace.rs`, and independently reran 56 tests
before its diagnostic retest. The debug and optimized native results follow below;
this diagnostics implementation does not itself change performance semantics.

Opt-in `ICHILOTO_GPUI_TRACE=1` adds:

- `submit_begin`, `submit_end` / `submit_failed`: around the existing blocking
  send, with instantaneous queued-update count and capacity.
- `dequeued`: immediately after the existing foreground `recv().await`, with
  queue count/capacity; `replace_begin` immediately before assignment.
- `elements_built`: after the renderer creates the root subtree, before returning
  it to GPUI. The earlier `render_callback` entry marker is retained.
- `layout_request_begin/end`, `prepaint_begin/end`, `paint_begin/end`: around the
  root's public GPUI Element lifecycle methods. The delegating wrapper preserves
  element ID, source location, associated state types, bounds and arguments.
  Disabled observations return the original unwrapped element. Request-layout
  hooks surround tree construction, not the entire Taffy computation; computation
  can occur in the gap before prepaint. `paint_end` marks CPU subtree paint return,
  not Metal submission/completion or visible pixels.

Lifecycle timings are monotonic elapsed durations around CPU-side methods. They
include any waits or preemption inside those methods; they are not per-thread CPU
accounting. “CPU paint” below names the observed stage, not measured CPU usage.

Queue count is a point observation, not an atomic enqueue/dequeue history.
The consumer can dequeue/replace before the producer records `submit_end`.
Use same-producer begin/end for submission duration and actual dequeue timestamps
for foreground progress; do not infer a negative queue wait from log ordering.
All new records retain renderer-local sequence plus source protocol/frame identity.
Diagnostics remain bounded, nonblocking and stderr-only. Existing stdout schemas,
complete-frame ordering, two-slot capacity, input, close and scheduling semantics
are unchanged. No dependency was modified.

[Checks](evidence/burst-investigation/candidate-checks) pass:56 Rust tests,
format check, all-target locked Clippy with warnings denied, and locked build.
The new deterministic test exercises a full two-slot channel before releasing
capacity, ordered v1/v2 delivery including repeated/decreasing frame numbers,
submission failure on receiver close, and enabled diagnostic identity/capacity
records. Disabled diagnostics do not query the queue. No test asserts an arbitrary
millisecond threshold. Existing upstream future-compatibility notices for `block`
and `proc-macro-error2` remain.

## Baseline closure

The baseline window closed normally, returned 0 and restored exact stty state.
`ps` confirmed Console 7629, PHP 7644 and renderer 7655 all absent.
See [completion](evidence/burst-investigation/baseline/completion.json), including
the preserved full raw-log path and SHA256. Both subsequent native passes used
fresh private muted runtimes and closed before binary staging or another launch.

## Staged diagnostic debug run: causal localization

The user subsequently approved replacement of only the ignored installed binary;
Engine staged the reviewed `58cfd712…` candidate and preserved the original.
A fresh private runtime at `/tmp/ichiloto-renderer-burst-candidate-20260912` used
the identical ordinary Console command and verified muted settings. One native
window ran as Console 56882, game PHP 56896 and renderer 56906. After the same five
Return keys, Home was free at `(8,4)`.

Sample D repeated Right/Left/Right/Left during a nominal 5-second, 1ms native sample,
with the same 1.5-second offset after its collecting banner. E repeated the same
sequence without sampling. [Candidate evidence](evidence/burst-investigation/candidate-debug)
includes exact commands, all raw observations, sample output and request brackets.
All 14 keys match PHP parsing; all 192 complete frames reached replacement; both
clock anchors intersect in `[153592512357624,153592512358291]`ns (667ns wide).
The final extra Down prepared a separate manual gate and is excluded from bursts.

The analysis now pairs **every** chronological lifecycle begin/end occurrence,
retaining repeated render callbacks and stages instead of combining first/last
entries from different invocations. It separately records request-layout duration
and the gap to prepaint. [Span correlation](evidence/burst-investigation/candidate-debug/span-correlation.json)
contains all paired spans, exact nanoseconds and overlaps; no begin/end stage was
left unpaired in this completed run.

| Burst / frame | Acceptance→dequeue | Blocking submit | Overlap with previous measured subtree stages | Interpretation |
| --- | ---: | ---: | ---: | --- |
| D / 138 | 178.717ms | 0.003ms | 174.219ms, frame 137 | Frame already submitted; foreground still drawing 137 |
| D / 139 | 144.782ms | 0.007ms | 140.362ms, frame 138 | Frame already submitted; foreground still drawing 138 |
| E / 163 | 140.781ms | 0.009ms | 136.633ms, frame 162 | Same symptom without the sampler |
| E / 164 | 115.880ms | 0.003ms | 111.719ms, frame 162 | Another complete snapshot waits behind the same draw |
| E / 165 | 89.493ms | 89.463ms | 85.363ms, frame 162 paint | Two slots filled; reader backpressure is a consequence of foreground drawing |

For D, frame 137 takes 7.731ms to construct elements, 21.485ms in request-layout, 12.519ms between request-layout return and prepaint, 10.468ms in prepaint and
122.016ms in root CPU paint. Frame 138 takes 7.114/23.388/11.759/10.556/104.462ms
respectively. Actual snapshot assignment takes about 0.006ms. The measured
subtree spans account for approximately 97% of each of D's two large waits.
Capacity 2 is observed; D's two problematic sends each see depth 0 before and 1
after submission. Enlarging the channel cannot remove the main-thread draw work.

Sample D's main thread has 2717/3506 observations in normal AppKit Mach-port wait,
GPUI draw/Taffy/layout/quad/text/bounds-tree paths during drawing, and 283 semaphore
wait observations under `Window::present` → Metal `next_drawable` →
`CAMetalLayer nextDrawable`. Those Metal observations are aggregate evidence,
not assigned to either individual delay: only about 4.4ms of each target interval
lies outside the explicitly measured subtree spans. No GPU visibility claim is
made. The reader spends 3500/3506 observations in stdin `read`, plus a few parser,
file and PNG/decompression observations. Unlike baseline A/B, PNG work **is**
observed in D, before acceptance; it does not explain these later long intervals.
No image-cache rewrite is justified.

**Updated causal conclusion:** the unoptimized candidate's foreground subtree
construction/layout/CPU paint delays subsequent snapshot handling, with paint the
largest measured component. Channel backpressure can follow when its two slots
fill. This is not evidence for a broken executor or insufficient queue capacity.
The debug build is unoptimized, so the next controlled experiment compiles the
**same source** with `cargo build --release --locked`; this is a build-profile
comparison, not an algorithm-fix before/after. Engine agreed to this comparison
before any renderer architecture change.

A physical held Right request specified the muted Last Legend title bar, about
one second of hold, release and the completion response “Released” (or “Skip”).
Automated input stopped during the bounded wait. No response or native key arrived,
so the window was closed normally instead of waiting indefinitely. The genuine
hold/release gate remains unperformed; wall-stopped movement is not accepted as
release behavior. Exit 0, exact stty restoration and no remaining 56882/56896/56906
processes are recorded in [completion](evidence/burst-investigation/candidate-debug/completion.json).

## Same-source optimized comparison and final validation

`cargo build --release --locked` produced
`eda9ec05b582bcbbe90a67b19910fdb7d6e4a0ec9f7907b013a58fa7ce7cbdd9`.
There are no custom Cargo profile settings or runtime-source differences from the
reviewed debug candidate. The [source/lock hashes](evidence/burst-investigation/candidate-release/source-SHA256SUMS)
identify that source. The initial sandboxed release build could not write the
normal macOS Metal compiler cache; the approved native-cache retry completed.
Both build logs are preserved. Engine retained both earlier binary backups and
staged the optimized executable through the same ignored installation boundary.

One fresh ordinary Console run used private muted runtime
`/tmp/ichiloto-renderer-burst-release-20260912/runtime`, Console 60545,
game PHP 60559 and GPUI 60569. Sample F and unsampled G each repeated exactly
Right/Left/Right/Left at free Home `(8,4)↔(9,4)`. F used the same nominal 5-second, 1ms sample and 1.5-second offset after its collecting banner. These are matched
build-profile observations, **not a renderer source-fix before/after**.

| Series / key | Frame | Native→receipt | Receipt→accepted | Accepted→replaced | Replaced→callback | CPU paint |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| F / 6 Right | 134 | 8.488ms | 0.404ms | 0.022ms | 8.215ms | 9.926ms |
| F / 7 Left | 135 | 32.546ms | 0.773ms | 2.033ms | 0.009ms | 8.481ms |
| F / 8 Right | 136 | 54.050ms | 0.207ms | 0.015ms | 12.032ms | 9.440ms |
| F / 9 Left | 137 | 73.282ms | 0.206ms | 8.807ms | 0.010ms | 8.626ms |
| G / 10 Right | 146 | 24.067ms | 0.776ms | 0.019ms | 3.440ms | 8.605ms |
| G / 11 Left | 147 | 32.240ms | 0.212ms | 0.017ms | 12.403ms | 8.495ms |
| G / 12 Right | 148 | 53.516ms | 0.221ms | 6.598ms | 0.895ms | 7.940ms |
| G / 13 Left | 149 | 78.362ms | 0.249ms | 0.018ms | 15.693ms | 7.897ms |

All eight snapshots receive their own callback and final coordinates are correct.
F's request-layout spans are 1.805–2.096ms, the gap to prepaint 1.194–1.398ms,
prepaint 1.370–1.869ms and construction 1.267–1.901ms. No stage crosses invocation
boundaries in the chronological analysis. Compared with the debug candidate,
the optimized build materially reduces both the foreground drawing cost and the
resulting handoff wait. This addresses the measured renderer symptom without a
capacity, repeat, camera, frame-ordering or drawing-algorithm change.

The [optimized sample](evidence/burst-investigation/candidate-release/home-f-sample.txt)
contains 3588/3724 main-thread observations in normal AppKit Mach-port wait, with
shorter drawing paths. The reader is in ordinary stdin `read` for 3723 observations
across two stack branches, plus one other observation. There is no reported
`send_blocking` or main-thread drawable semaphore wait in this collection. These
remain aggregate stacks; precise per-frame costs come from the paired lifecycle
spans, not inferred timestamps within the stack report.

The final key's native→callback remains 82.305ms in F and 94.322ms in G. Most of
that interval precedes renderer receipt (73.282/78.362ms). Engine's sequential
input consumption/cadence remains a separate contributor; this work changes
neither FIFO behavior nor gameplay timing. CUA request times and native callback
arrivals remain distinct. No physical burst/hold/release timing has been validated,
and these limited Home observations do not constitute a performance guarantee
for every map or battle.

The ordinary optimized run independently verified:

- Uppercase M opens the real coloured map and C returns to Home.
- F5 displays Quick Saved and writes only the private save.
- Equipment Tab changes Kaelion/Vanguard to Liora/Oracle; Shift+Tab restores it.
- Native resizing from 1350×720 content to 900×480 uniformly scales at 2/3.
  Menu text and the field sprite remain aligned. Enlargement to 1550×800 retains
  the 1x cap and centers the 1350×720 surface with offsets 100,40.
- A separate post-resize RL RL check still returns `(8,4)`, west; no lingering
  motion or stale final surface was seen. It is excluded from the matched timings.
- Normal native close exits 0, restores exact stty state, and leaves no
  Console 60545/PHP 60559/GPUI 60569 process.

Screenshots, full repeated observations, exact IDs/timestamps and commands are in
[candidate-release](evidence/burst-investigation/candidate-release). All 29 keys
match PHP parsing; all 203 frames are received, accepted, submitted, dequeued and
replaced. Both anchors intersect in `[154353951986750,154353951986791]`ns (41ns
wide); no record is dropped and no stderr error occurs. The source manifest and
all three run directories include checksums. Full original logs remain private
under their documented `/tmp` paths with SHA256 receipts; extracted relevant
records and complete renderer streams are archived here.

Validation is complete for this renderer change: 56 Rust tests in both debug and
release profiles; formatting; locked all-target Clippy with warnings denied;
locked debug/release builds; thirteen sequential silent native v1/v2 IPC cases
with tracing disabled and another thirteen with tracing enabled. The smoke tool's
new `--trace` option validates diagnostic stderr separately while preserving
stdout/event assertions. It covers full/style/empty replacements, mixed/malformed
traffic, close/shutdown and bounded output backpressure. Native process cleanup
was checked after both suites. These IPC tests do not claim pixel comparisons.

The practical correction is documented in README: build and stage
`target/release/gpui-renderer` for gameplay/performance validation; retain debug
builds for development/debugging. Do not compare their timings as though both
were optimized runtimes. No production scheduling or rendering refactor is needed
for this diagnosed case. Engine may retain the internally staged optimized binary.
Physical held-input gameplay, an eligible T skit, battle and full S7-E acceptance
remain explicitly open under Engine coordination.
