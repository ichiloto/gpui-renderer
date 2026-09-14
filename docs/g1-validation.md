# G1 native canvas validation

14 September 2026. The native canvas slice is implemented and validated on
macOS/Apple Silicon with published GPUI 0.2.2. Real-game G1 battle acceptance
remains with Engine/Game and approved Art; this record does not claim it.

## Implemented boundary

Negotiated v2 `graphical_canvas` adds an independent logical canvas, resolved
image rectangles, optional crops/opacity, explicitly positioned indicators and
local-grid text. PHP owns geometry, identity and state. Native fitting centers
the complete canvas uniformly, with the current maximum scale of one.

Every frame replaces the complete scene. Omitted canvas returns to legacy
presentation; null and mixed nonempty legacy/canvas payloads are rejected.
Canvas null backgrounds are transparent; explicit colors paint cells. Existing
legacy sprite, tile and text meanings are unchanged.

Whole and cropped canvas images share the existing guarded-region cache.
Original decoded images and prepared regions retain their separate 64 MiB
budgets, including guard bytes. This fixed a thin neighboring-image color line
observed at the arena edge during the initial installed-window inspection.
Regions are reused across frames; there is no image decode/crop per paint.

## Shared fixtures and automated checks

The [canonical corpus](../fixtures/graphical-canvas/manifest.json) contains
139 exact payloads, including 18 accepted cases. Its manifest SHA256 is
`1cc3acd92570105e13eced9d4fc9d81dcea004dc666d8331aca5ad2897635790`.
[SHA256SUMS](../fixtures/graphical-canvas/SHA256SUMS) covers all payloads and
synthetic PNGs, including the existing parent test-sprite.png.

Engine confirmed byte-identical copies and matching PHP emission after the
documented omitted-list/default-opacity normalization. Native tests separately
classify JSON, schema, session and image-preparation rejection stages. They
retain an actual prior prepared display when rejecting input.

- 93 optimized Rust tests passed, including all 85 prior renderer tests.
- `cargo clippy --locked --all-targets -- -D warnings` passed.
- Formatting, diff whitespace and every frozen fixture hash passed.
- New tests cover independent/fractional geometry, ordering, instance removal,
  full clearing, crop/whole-image edge isolation and reuse, decoded/guarded cache
  budgets, invalid numeric values and asset-root/symlink confinement.

Dependency future-compatibility notices remain for existing `block` and
`proc-macro-error2`; no dependency or compiler policy was changed.

## Installed native check

Engine staged the optimized executable through its existing manifest and
PackagedRendererExecutableResolver. Manifest and app metadata were preserved.
The verified installed SHA256 is
`31521f61fc5d6dcab7231e906612cc5595f6af93c2841afbf1dfc80f11cf07ca`.

One silent installed fixture process was active at a time. CUA inspected the
actual app at 320x320 content size and after native zoom to 1920x1062. The
1350x720 canvas fitted at approximately 0.237 scale in the small window and
at scale one with offsets (285,171) in the large window. The same transform
aligned background, images, markers and text.

Visual inspection confirmed the corrected clean arena edge, a whole image at
(969,265) sized 143x181, and a cropped image at fractional (729.5,224.5) sized
143x214.5. Opaque command cells, transparent feedback, a selection outline and
a separately positioned acting underline were visible and correctly layered.
These were synthetic geometry fixtures, not Last Legend artwork.

The [native receipt](evidence/g1/native-receipt.json) records negotiated
capabilities, five CPU paint completions and exit zero. The sequence used
3,1,1,0,0 canvas images, then returned to legacy text. Resource and viewport
observations confirm the canvas collections cleared and logical sizing returned
to the 320x320 legacy grid. Final clearing pixels were not captured before timed
close; clearing evidence is the atomic-state tests and native CPU observations.
The owned process exited normally and its window closed.

The reusable check is `scripts/native-tile-smoke.py --canvas`; the optional
observation period permits inspection and then automatic shutdown. No game,
save, audio or gameplay state is used. No FPS/GPU-completion measurement,
Linux/WSLg validation, real battle outcome or G2 delivery is claimed here.
