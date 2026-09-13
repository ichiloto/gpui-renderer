# Stable graphical viewport based on the battle screen

2026-09-12. The graphical default is the full built-in battle footprint:
**135 columns × 36 rows**, shared by every scene. This replaces automatic sizing
from the launching terminal. The field occupies 135 × 30 cells and the command,
context, names and status panels occupy the remaining six rows. The 110 × 35
main menu fits within this same viewport; it is not the size reference.

With the registered GPUI cell dimensions of 10 × 20, the default logical surface
is 1350 × 720. The native window initially fits that surface within the display's
usable area. Physical window resizing scales the whole surface uniformly when
needed, preserving battle controls and the relative geometry of images/text.
Larger maps scroll through the fixed logical viewport. Scene changes neither
resize the surface nor fit it to whatever content happens to be visible.

Engine owns this change. It preserves original per-axis size requests separately
from automatically resolved dimensions, so attaching a renderer after Game
construction also selects the graphical default. Settings, Console and all
registered scene cameras are synchronized. Inactive scenes are not started or
redrawn simply to update their camera viewport.

Explicit dimensions still take precedence, including flat/nested constructor
options and non-default positional dimensions. For example:

```php
new Game('My Game', options: ['screen' => ['width' => 160, 'height' => 40]]);
```

These are logical cells. Native window size and display fitting remain separate.
Terminal sessions retain automatic terminal sizing and resize behavior. This
change does not add a fullscreen/fixed-window settings interface or impose a
mandatory non-resizable window.

## Verification

The focused launch/geometry checks passed 71 tests with 421 assertions. They
include 16 real constructor scenarios: small/large parent terminals, explicit
flat/nested/positional/per-axis requests, repeated configuration, later renderer
attachment and precedence over invalid launch intent. Assertions check the
actual PHP renderer-stub hello, all five scene cameras, settings, options and
Console dimensions. Both 80 × 24 and 220 × 60 launching terminals produce the
same default 135 × 36 graphical grid.

The full Engine suite passed **1,349 tests, one skipped, 5,143 assertions**;
PHPStan reported no errors. Renderer runtime source is unchanged from the
validated S8/readability candidate
`20a38d04d44f7eb0dd3469dadabdc2b7cf567487bac14db34bd73608bfa6a584`.
Its existing viewport tests already cover the exact 1350 × 720 surface,
centering, downscaling and invalid/minimized sizes. No Rust rebuild or protocol
change is required for Engine to select a different session grid.

Test receipts are retained in [the evidence directory](evidence/battle-viewport/receipts.json).

A single muted native probe launched through the ordinary CLI from a real
**220 × 60** parent terminal, with only FPS specified by the Game constructor.
The installed GPUI renderer automatically opened a **1350 × 720** content area.
Battle, menu, Garden field, a PHP camera pan and battle again all recorded
**135 × 36** for Console and the active camera; terminal output stayed disabled.
Engine's native inspection confirmed the complete battle borders and bottom
controls, all four menu party members and the changed map region after panning.

Native window zoom enlarged the content area to **1920 × 1062**, keeping the
1350 × 720 canvas centered at scale 1, with offsets (285, 171). Restoring the
window returned to 1350 × 720. Every recorded viewport retained the same
presented canvas dimensions. This checks automatic size selection and scene
composition; the probe held simulation between phases and was not a full battle
playthrough or a physical movement-input test. Smaller-window downscaling is
covered by the existing renderer tests, rather than this native zoom check.

The probe exited normally with status 0, empty stderr/error logs and unchanged
parent terminal modes. Engine confirmed the renderer was closed afterwards.
The [native summary](evidence/battle-viewport/native-summary.json), phase records,
driver and original renderer stderr records are retained with source receipts.
Screenshots were inspected by the Engine coordinator during the run; no image
files are included in this evidence directory.
