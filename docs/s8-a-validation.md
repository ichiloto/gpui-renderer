# S8-A renderer: sprite-sheet source rectangles

Status: implementation and noninteractive checks complete on macOS / Apple
Silicon, 2026-09-12. Changes are deliberately uncommitted for Engine review.
Engine owns Player animation, installation and ordinary-game acceptance. This
slice adds no animation state, tilemaps, NPC/battler behavior or repaint policy.

The subsequent [shared readability fix](readability-validation.md) retains this
implementation and adds a global base palette and measured UI text sizing. The
candidate hash and 64-test records below identify the initial S8-A build; the
linked report identifies the newer combined candidate and its validation.

## Negotiated contract

Both protocol versions accept optional hello
`requiredCapabilities:["sprite_source_rect"]`. The corresponding ready includes
`capabilities:["sprite_source_rect"]`. Callers must verify the acknowledgment
before sending crops. Unknown/duplicate requests reject hello. An older renderer
rejects the new hello field, and a missing acknowledgment must fail client startup.
There is no silent whole-sheet fallback.

Missing or empty requirements preserve the legacy ready shape. Whole-image frames
work with or without the capability. A sprite with `sourceRect` requires the
capability; omission preserves the old full-image/contain rendering behavior.
Explicit null is malformed. No protocol version was added.

```json
{"protocol":2,"type":"hello","title":"Last Legend","assetRoot":"/absolute/path/to/assets","grid":{"columns":135,"rows":36,"cellWidth":10,"cellHeight":20},"requiredCapabilities":["sprite_source_rect"]}
{"protocol":2,"type":"ready","capabilities":["sprite_source_rect"]}
```

Each optional sprite `sourceRect:{x,y,width,height}` uses source-image pixels,
not cells or destination pixels. All members are strict JSON unsigned 32-bit
integers. Origins are nonnegative, extents positive, sums checked for overflow,
and right/bottom edges must not exceed the decoded PNG dimensions. Invalid crops
reject the entire snapshot, preserving the previous accepted state.

Destination `x/y`, `width/height`, bottom-center anchor and layer retain their
existing meaning. Crops fill that destination; a non-square source sheet does not
change the player's feet or destination size. PHP supplies the current rectangle
and owns movement/elapsed-time/idle-frame selection.

## Rendering and cache ownership

The renderer positions a full `RenderImage` behind a destination-sized clipped
GPUI div. Full-sheet position is `(-sourceX * scaleX, -sourceY * scaleY)` inside
that div, where scales are destination extent divided by selected source extent.
The sheet uses `ObjectFit::Fill`; GPUI's GPU content mask clips the selection.
The normal viewport transform applies to both clip and sheet. No cropped buffers,
generated frame files, dependency patches or per-frame PNG decoding are needed
when the cached sheet is unchanged.

The prior implementation cached only within a frame. The new session LRU cache
holds at most 64 MiB of decoded full images and 1024 entries. Canonical path and
file length/mtime identify a cached image. A metadata change reloads; content edits
that preserve both length and timestamp require restarting the session. Paths are
still resolved and validated before a cache hit. Absolute/escaping paths, deleted
files and invalid PNGs retain their existing rejection behavior.

Each prepared snapshot retains its bounded cache generation. Queued and currently
displayed generations may keep older images alive in addition to the current
cache, so 64 MiB is not a process-wide memory claim. The existing per-frame 64 MiB
image budget and two-update channel remain. At the next render, atlas entries no
longer in that generation are removed with GPUI's public `Window::drop_image`.
They are not removed while the prior snapshot is still the rendered scene.
Crop-only changes retain the same image ID and atlas texture.

## Validation

All **64 Rust tests pass** in debug and release. New coverage includes:

- Strict capability requests, opt-in gating in v1/v2, exact legacy ready fields,
  and successful whole-image frames after rejected unnegotiated crops.
- Complete integer rectangle parsing, unknown/null/fractional/non-finite values,
  checked-sum overflow, edge-aligned rectangles and actual-image overflow.
- Distinct crops on one sheet in a single frame and successive same-number
  snapshots, sharing the full decoded image; whole-image regression; BGRA/alpha.
- Rejected crops preserving the last snapshot, world/sprite/UI ordering and
  existing colour-only/full-frame behavior.
- Canonical-path reuse, file metadata reload, retained old image bytes, and
  deterministic byte/count LRU eviction without timing thresholds.
- A 1280×1024 sheet crop and bottom-center feet through 1×, 0.75× and 0.5×
  viewport scales, including centering and exclusion of neighboring source cells.

Commands passed:

```sh
cargo test --locked
cargo test --release --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
cargo build --release --locked
git diff --check
```

Existing upstream future-compatibility notices for `block` and
`proc-macro-error2` remain. Release executable SHA256:
`dfcccf3e91c3b32e3ddcda2fcc0c502895623bd3c683609e5e173cdf954a6c5e`.
Source/build receipts are in [evidence](evidence/s8-a).

Read-only PNG-header checks of the supplied Kaelion sheets confirm down/left/right
1280×1280 and up 1280×1024. This task did not copy, modify or vendor those assets.
Authored frame counts and timing remain Engine's responsibility.

No graphical windows were launched for this slice. These tests verify parsing,
preparation, cache ownership and crop geometry; they do not constitute native
pixel or gameplay-animation acceptance. Engine must validate the candidate in
its ordinary game integration, including visible crop/alpha boundaries and resize.
Deferred S7 repaint/repeat work is unchanged.
