# Shared renderer readability

2026-09-12: renderer implementation, noninteractive checks and bounded Engine-owned
native verification complete. All S8-A source-rectangle,
capability, image-cache and alpha-compositing work remains in the candidate.
Changes remain uncommitted; no renderer-task UI was launched.

## Scope and diagnosis

The reported screenshots show poor blue visibility and undersized glyphs inside
fixed cells. The old base palette mapped normal blue to `#000080` and bright blue
to `#0000FF` on default `#111820`. Their sRGB luminance contrast ratios were about
1.12:1 and 2.08:1. The old 10×20-cell font used a 9px em, rather than measuring
the actual glyph advance; a typical monospace glyph occupied only about half the
cell width.

This is shared presentation work for dialogue, menus and HUD text as well as
text layers in a scene. It does not depend on a particular map or ASCII character,
and remains relevant when the world uses image tiles. It adds no tilemaps or
automatic recolouring of authored images. Image crop, scale, anchor, layer and
viewport geometry are unchanged. No Engine, Console, game or dependency source
was edited by this task.

## Palette and explicit colours

`color.rs` defines a coherent dark-terminal base palette, reused by ANSI16 and
ANSI256 indices 0..15. Normal blue is now `#61AFEF` (7.56:1 against the default
background), and bright blue is `#8CCAFF` (10.22:1). All indices 1..15 exceed
4.5:1; normal chromatic colours range from about 5.59:1 to 10.34:1. Bright
variants have greater luminance than their normal counterparts.

Black remains literal black. Default foreground/background stay `#D9E1E8` and
`#111820`. ANSI256 cube/grayscale indices 16..255 and explicit RGB are unchanged.
An explicit background uses the same colour identity mapping as a foreground;
spaces remain opaque. There is no contextual colour filter, sprite tint, gamma
change or per-frame contrast adjustment. This theme cannot guarantee contrast
for arbitrary authored foreground/background or image-pixel combinations.

Tests compute relative sRGB luminance and contrast ratios, enforce the theme's
readability target, retain distinct bright variants, and verify authored RGB,
cube colours and explicit background semantics. They do not merely restate new
hex constants.

## Text metrics

The renderer resolves its selected Menlo/monospace font through public GPUI
`TextSystem::resolve_font`, then measures `ch_advance`, `ascent` and `descent`
at a 1px reference em. The logical font size is:

```text
min(0.95 * cellWidth / advancePerEm,
    0.95 * cellHeight / (ascentPerEm + descentPerEm),
    cellHeight)
```

It is cached once for the session and multiplied by the existing viewport scale.
Cell positions and line height still use the unchanged logical grid. The small
inset allows glyph edges to fit without changing layout or applying boldness.
Metrics errors/non-finite or nonpositive values use the prior bounded fallback.
All text layers share this path, including ordinary graphical-game UI.

Representative monospace metrics of 0.6em advance and 1.2em line extent yield a
15.83px em in a 10×20 cell instead of 9px. This example is a deterministic
geometry test, not a claim about the font measured in a particular native run.
Opt-in `text_metrics` diagnostics report the actual metrics and chosen size for
Engine's native review. Tests cover narrow/tall cells, height-constrained cells,
resizing, unchanged cell pitch and invalid/extreme metrics.

## Candidate and verification

All **68 tests pass** in both debug and release, including the earlier S8-A crop,
cache, layering, alpha, full-frame and input tests. Locked all-target Clippy with
warnings denied, formatting, diff checks and the locked release build pass.
Existing upstream future-compatibility notices remain.

Release candidate SHA256:
`20a38d04d44f7eb0dd3469dadabdc2b7cf567487bac14db34bd73608bfa6a584`.
The prior S8-A executable `dfcccf3…` has the old palette and font sizing. The
reported 15:51 readability screenshot was captured before this new candidate
was installed; Engine confirmed it was a baseline capture and closed it.

[Evidence](evidence/readability) contains test/build output, exact raw-output
receipts and source hashes. Earlier S8-A receipts remain historical records of
the prior candidate.

## Engine-owned native verification

Engine verified the installed `20a38d04…` candidate and tested one private, muted
ordinary game session at a 135×36 logical grid. Its native observations found
substantially more legible title/load-save screens, Town Center labels and water,
HUD and four-character party menu. Engine then entered Home, reached Mother at
`(22,5)` facing east through normal input, and observed the completed wrapped
dialogue: “The shop in Town Center should have an S-Mana wafer.”

Kaelion's authored PNG colours remained unchanged. Sprite crops showed no sheet
leakage, and the menu hid/restored field sprite ownership. Engine closed the window;
the native app reported not running, process 23304 exited 0, the error log was empty,
and terminal settings matched byte-for-byte before and after the run.

These observations were supplied by the Engine coordinator. Native screenshots
are inline in the Engine task and were not persisted as local image files. GPUI
tracing was enabled, but Engine tracing was not, so diagnostic stderr and the
actual `text_metrics` values were not retained. The 15.83px example above remains
a synthetic metric example, not a measured native font size.

This acceptance covers shared text/themed-colour legibility and the stated crop
and menu regressions. Authored image/background contrast remains unchanged and
is not declared resolved. Live resizing and other platforms were not manually
tested in this follow-up; resize geometry remains covered by the automated tests.
No additional runtime edits or UI launches followed this verification.
