# Matching renderer installation and native startup

2026-09-13, 14:34:59 UTC. The author authorized replacing the local installed
renderer. This supersedes the installation hold in the original
[S8-B validation record](s8-b-validation.md), without merging any source branch.

## Failure and correction

The Engine's normal GPUI registration requires `sprite_source_rect` and
`tile_batches`. Its installed manifest still selected the older executable
whose capability parser knew only `sprite_source_rect`. The reported
`unknown variant tile_batches` was therefore a startup compatibility failure.
The subsequent exit 143 recorded Engine's cleanup, rather than a separate
renderer crash.

The renderer source was already committed at
`40000860fa9202eef215e886d9857bf56d63686f` on `feat/s8-b-field-tiles`.
All Rust/Cargo inputs matched the existing source receipt. Rebuilding with
`cargo build --release --locked --offline` reproduced the reviewed executable;
all **81 optimized tests** passed again. No protocol or capability requirements
were weakened to accommodate the old installation.

| Executable | SHA256 |
| --- | --- |
| Previous installation | `20a38d04d44f7eb0dd3469dadabdc2b7cf567487bac14db34bd73608bfa6a584` |
| Built and installed replacement | `f3b4f56f289ac1bb60a03c25db1e143f3425617d35b6b3f59e42a8a6ca38e8d7` |

The existing Engine packaging boundary was used:
`resources/renderers/installed/manifest.json` resolves GPUI to
`gpui/darwin-arm64/Ichiloto Renderer.app/Contents/MacOS/gpui-renderer`.
The complete old app and manifest were backed up beneath the ignored installation
directory at `.backups/gpui-20260913T143459Z`. A verified temporary executable
was atomically renamed into place with mode 0755. The manifest and app's
`Info.plist` were preserved and rehashed. Engine's actual
`PackagedRendererExecutableResolver::resolve('gpui')` then resolved the new hash.
The release executable's existing ad-hoc signature verifies; this is local
development staging, not a notarized distribution release.

Rollback can atomically restore the backed-up executable at the same manifest
path. That restores the old installation but also restores its incompatibility
with an Engine requiring terrain tiles.

## Native check

No renderer process was running before the announced test. One silent, owned
process was launched against the installed bundle executable, using repository
fixtures and no game, audio, player input or save data. Its real stdout returned:

```json
{"protocol":2,"type":"ready","capabilities":["sprite_source_rect","tile_batches"]}
```

The check waited for each frame's `paint_end` observation before sending the next
full replacement. It exercised terrain with a cropped actor and text/UI,
a full **4,860-cell** terrain viewport, removal of all terrain, and restoration.
The four frame resource records contained **2, 4860, 0, 2** terrain cells.
The process exited normally with status **0**, with no protocol errors and
zero dropped diagnostic records. The window closed before Engine began its
separate ordinary launch-path validation.

These observations verify native startup, preparation and CPU painting. They
do not measure GPU completion, FPS, real gameplay, collision or Garden scrolling.
Ordinary CLI discovery and runtime startup are validated separately by Engine;
full gameplay acceptance remains a separate workload.

The reusable [single-window check](../scripts/native-tile-smoke.py) accepts a
packager-resolved executable and an evidence directory:

```sh
python3 scripts/native-tile-smoke.py --binary /path/to/installed/gpui-renderer --evidence-dir /tmp/tile-smoke
```

It enforces a 20-second observation/write deadline and terminates only its owned
child on failure. [Installation and native receipts](evidence/s8-b-install/)
retain the hashes, actual ready event, resource records and paint diagnostics.
The existing frozen fixtures reconstruct the input; its exact capture hash is
recorded in `installation.json`.
