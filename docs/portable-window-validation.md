# Portable native window validation

Historical checkpoint: 14 September 2026. This correction was subsequently
integrated in `b44b08a8393ab74c28a0ff5c947274c5cbfca4f1` and is included in the
accepted normal Engine installation. Current status and workflow are maintained
in the [README](../README.md#availability-and-installation).

## Behavior

Native startup no longer rejects Linux or Windows. Every GPUI backend uses the
same policy: request maximization from the desktop, retain centered restore
bounds fitted to the display's suggested default window bounds, and fit the
immutable game grid or canvas to the actual content viewport on every paint.
Players retain the normal restore, move and resize controls. The initial window
now requests maximization on macOS as well as other backends.

Suggested display bounds are only restore hints. They are not advertised as usable
work-area measurements. The window manager supplies actual geometry and may apply
its own placement policy; rendering always follows the resulting content size.
No fixed taskbar allowance, private GPUI API, protocol change or dependency change
was introduced. Existing unused direct cocoa/objc dependencies remain under the
coordinator's instruction to preserve manifests in this correction.

## Pinned backend evidence

Inspected the published `gpui-0.2.2` source, not a newer Zed checkout:

- `src/platform.rs`: public `WindowBounds::Maximized` stores restore bounds.
- `src/window.rs`, `Window::new`: maximized startup invokes the platform's `zoom`.
- `src/platform/mac/window.rs`: `zoom` requests AppKit window zoom.
- `src/platform/linux/wayland/window.rs`: `zoom` requests `set_maximized`.
- `src/platform/linux/x11/window.rs`: `zoom` requests both EWMH maximized axes.
- `src/platform/windows/window.rs`: `zoom` uses `SW_MAXIMIZE` or records maximized
  initial placement before the window is visible.

This establishes the selected API path, not execution proof on those platforms.

## Validation

- 95 optimized Rust tests passed. Portable initial-geometry cases cover small,
  portrait and fractional bounds, off-origin displays, centered bounded restore
  geometry, and invalid display metadata. Existing viewport, image, tile, canvas,
  replacement and protocol tests remain included.
- Formatting, Clippy across all targets with warnings denied, release build and
  diff whitespace checks passed on macOS/Apple Silicon. Clippy's all-targets flag
  covers Cargo target kinds on this host, not other operating systems.
- One silent synthetic native process used a temporary app bundle; Engine's
  installed binary and Game checkout were unchanged during this checkpoint.
- CUA visually checked initially maximized canvas presentation at 1920x1062
  content size (scale 1, offsets 285/171), restoration to 320x320 (scale about
  0.237, offsets 0/74.667), and resizing the cleared legacy return to 763x562
  (320x320 logical grid, offsets 221.5/121).
- All 46 observed viewport transitions were finite, centered and fitted. Five
  canvas/replacement frames painted, then normal shutdown exited zero. The owned
  window closed. CPU paint observations are not GPU completion or FPS.

The [receipt](evidence/portable-window/native-receipt.json) includes the executable
hash, changed-source hashes, startup request and selected actual viewport records.
The temporary bundle and raw scratch traces were removed during the
16 September cleanup; the committed receipt remains the durable evidence for
this checkpoint. Later native resize evidence is retained with
[glyph-effects validation](glyph-effects-validation.md#focused-native-validation).

Linux/WSLg and native Windows were not compiled or run here. Real platform
builds and desktop checks remain necessary. Engine transport and platform-package
delivery are separate coordinated work: a GPUI backend existing does not establish
end-to-end game support. Pulling the Engine source does not install this executable.
