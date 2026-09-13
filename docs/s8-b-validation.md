# S8-B: tile batches and Garden scrolling

## Starting point

2026-09-13. Renderer `develop` started at
`6556e414c819f4cbd71b0a9865c4e9824e111426` with the existing S8-A/readability
source and validation work uncommitted. Every Rust/Cargo input matched the
readability source receipt. Both the existing release and installed renderer
hashed `20a38d04d44f7eb0dd3469dadabdc2b7cf567487bac14db34bd73608bfa6a584`.
The actual baseline passed **68 tests**, format checking and locked Clippy.
It is now preserved as the separate prerequisite commit `89c829f` on the
renderer feature branch, including its existing compact validation evidence.
Cargo reported existing future-incompatibility notices for dependency packages
`block` and `proc-macro-error2`. No baseline source was reset to an older commit.

Engine's graphical geometry commit is `b3e2a26`; its terminal cap prerequisite
is `daba789`, awaiting the Engine repository's required independent review.
Native integration is held until that prerequisite lands and matching Engine
and Game changes are ready. S8-B does not change viewport sizing policy.
The new Engine candidate requests `tile_batches` at startup. The still-installed
older `20a38d04` renderer cannot acknowledge it and therefore fails clearly;
there is no silent fallback. Install the reviewed matching release only as part
of coordinated integration after the prerequisite gate.

## Contract and implementation

The [frozen v2 contract](tile-batches.md) defines `tile_batches` negotiation,
strict DTO fields, independent terrain budgets, full replacement and ordering.
The [shared fixtures](../fixtures/tile-batches/manifest.json) contain 45 exact
payloads, including a complete 4860-cell viewport. Their manifest separates
wire parsing, capability gating, frame structure and decoded-image failures.
Engine consumes accepted payloads to verify its outbound serializer; it does
not need an invented inbound parser or PNG decoder to use these fixtures.

GPUI uses **one canvas/layout element per batch**, drawing each cell in payload
order with the same logical viewport transform as text and sprites. Each cell
still inserts an image primitive. This is a reduction in element/layout count,
not a claim that a protocol batch maps to one GPU draw call.

Source PNGs reuse the existing confined, canonical-path/metadata cache. Tiles
and actors share the per-frame **64 MiB / 1024 distinct source-image** budget.
Aliases of one canonical image are counted once. Existing PNG limits remain
16 MiB encoded, 4096 per dimension and 64 MiB decoded.

GPUI 0.2.2's public image painter rounds device bounds and samples linearly,
without a source-UV or per-image filter option. To isolate neighbouring tiles,
preparation derives a source-region image with two pixels of repeated edge
colour/alpha on every side. Regions are cached by decoded atlas identity and
source rectangle, shared across destination cells and subsequent frames.
There are no per-cell PNGs, per-frame crop allocations on cache hits or Rust
animation clocks. A changed atlas identity naturally produces new regions.

Derived regions have a separate **64 MiB / 4096-region** LRU bound and the same
per-frame byte/count ceiling. The byte accounting includes guard pixels; a
single region exceeding that ceiling is rejected before allocation. At most
4100 pixels per derived dimension follows from the source limit plus guards.
Live prepared snapshots retain immutable source/region references independently
of cache eviction. The bounded two-update queue and current displayed frame
can retain additional generations; these cache limits are not a process-RSS
claim. The UI tracks cached and actively referenced images, retiring obsolete
GPU entries on the next draw. Invalid frames never replace the accepted state.

Painting rounds both outer guard endpoints to device pixels, then clips to the
exact unsnapped destination cell. Tests check sampling footprints at fractional
scales, display scale factors and all viewport corners. The shared transform
continues to control text, actor crops and terrain. Existing actor sheet
painting and text/sprite tie ordering are preserved; equal layers put terrain
before text, then sprites, stably within each type.

Opt-in `frame_resources` diagnostics report tile batch/cell counts, distinct
decoded images/bytes, prepared region counts/bytes, PNG decodes and region
builds for each accepted frame. Existing lifecycle records still distinguish
preparation, CPU layout/prepaint/paint and queueing; none denotes GPU completion.

## Validation status

The candidate passes **81 debug tests and 81 release tests**, format checking,
locked Clippy and the locked optimized release build. The release binary SHA256
is `f3b4f56f289ac1bb60a03c25db1e143f3425617d35b6b3f59e42a8a6ca38e8d7`.
[Logs and source/binary receipts](evidence/s8-b/receipts.json) are retained.
No candidate binary has been installed and no native test window launched yet;
native acceptance and scrolling measurements remain pending, not measured zero.
The first shared-fixture test identified a filename collision between missing
`asset` field and absent asset file cases; it was corrected to distinct names.
Initial locked Clippy identified a test iterator style lint, also corrected.
These are recorded rather than silently hidden by subsequent runs.

Native acceptance must cover Garden terrain/animated Player, movement and
collision, camera edges, retained text/dialogue/UI, menu/scene clearing and
window resizing. Controlled camera pans must remain distinct from real input.
The comparison route is the recovered historical workload: seed 12345,
135 × 36, 10 × 20 cells, 12 idle iterations followed by `moveTo(i, i)` for
`i = 0..35`. Both comparison arms must be capped, buffer-only, normalized-SGR
and use optimized renderer builds. Older mirrored-terminal runs are not the
baseline for this slice.

All native checks remain muted and single-window. A previous denial of Native
Terminal UI automation must not be bypassed with another UI mechanism.

Game's local S8-B revision is `47b38a2ba5058aaa59b01cfb736b298c1828acca`.
It supplies a neutral 48 × 32 atlas with three 16 × 32 sources: `;` grass,
`~` water and `x` stone boundary; spaces stay text. The Game task records the
user-approved replacement of four ambiguous branch `x` markers with equally
solid `#` markers, preserving collision outcomes. Atlas SHA256:
`5b256f67fdf5da2edd245ab2e5d504d7a1b7ab6b36bbf721424ef95c7d92d0ea`.

The old temporary checkpoint no longer exists. The Game task verified a current
legitimate `.data/saves/quick/auto-02.iedata` through its normal compatibility
pipeline: Garden (8, 3), arrival and arrival cinematic complete. A byte-identical
private copy was prepared for later acceptance, SHA256
`7ab48eb3169d75009895da3abcdf187822ab45a82a64423a846db5ab49b88fba`.
No save payload was edited and no test runtime has been launched.
