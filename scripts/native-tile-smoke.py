#!/usr/bin/env python3
"""Check tile or canvas startup and painting in one brief, silent native window.

Requires a graphical desktop. Uses only repository fixtures; no game or saves.
Paint observations describe the CPU paint callback, not GPU completion or FPS.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import time

root = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--evidence-dir', type=Path, required=True)
parser.add_argument('--glyphs', action='store_true', help='Exercise negotiated runtime glyph contours, fades and clearing')
parser.add_argument('--canvas', action='store_true', help='Exercise the graphical canvas and return to legacy presentation')
parser.add_argument('--observe-seconds', type=float, default=0, help='Hold the first painted frame (each frame with --glyphs) for visual inspection (0..45 seconds)')
args = parser.parse_args()
if not 0 <= args.observe_seconds <= 45:
    parser.error('--observe-seconds must be in 0..45')
if args.canvas and args.glyphs:
    parser.error('--canvas and --glyphs are separate scenarios')
binary = args.binary.resolve(strict=True)
args.evidence_dir.mkdir(parents=True, exist_ok=True)
fixtures = root / ('fixtures/canvas-glyph-effects' if args.glyphs else 'fixtures/graphical-canvas' if args.canvas else 'fixtures/tile-batches')
hello = json.loads((fixtures / 'hello.json').read_text())
hello['assetRoot'] = str(root / 'fixtures')
hello['title'] = 'Ichiloto — renderer startup check (silent)'
events, diagnostics = [], []
raw = {'stdout': bytearray(), 'stderr': bytearray()}
pending = {'stdout': bytearray(), 'stderr': bytearray()}
inputs = []
deadline = time.monotonic() + 30 + args.observe_seconds * (4 if args.glyphs else 1)
process = subprocess.Popen(
    [str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
    stderr=subprocess.PIPE, bufsize=0,
    env=dict(os.environ, ICHILOTO_GPUI_TRACE='1'),
)
os.set_blocking(process.stdin.fileno(), False)
selector = selectors.DefaultSelector()
for stream, name in [(process.stdout, 'stdout'), (process.stderr, 'stderr')]:
    selector.register(stream, selectors.EVENT_READ, name)


def send(message):
    data = (json.dumps(message, separators=(',', ':')) + '\n').encode()
    inputs.append(data)
    # FileIO can perform partial pipe writes for a full viewport payload.
    view = memoryview(data)
    while view:
        if time.monotonic() >= deadline:
            raise TimeoutError('Renderer input exceeded the native check deadline')
        try:
            written = os.write(process.stdin.fileno(), view)
        except BlockingIOError:
            drain()
            continue
        view = view[written:]


def drain():
    for key, _ in selector.select(0.1):
        data = os.read(key.fd, 65536)
        if not data:
            selector.unregister(key.fileobj)
            continue
        name = key.data
        raw[name].extend(data)
        pending[name].extend(data)
        while b'\n' in pending[name]:
            line, _, rest = pending[name].partition(b'\n')
            pending[name] = bytearray(rest)
            record = json.loads(line)
            (events if name == 'stdout' else diagnostics).append(record)


def wait_for(predicate):
    while not predicate():
        if time.monotonic() >= deadline:
            raise TimeoutError('Native check exceeded its bounded deadline')
        drain()
        if any(event.get('type') == 'error' for event in events):
            raise RuntimeError(f'Renderer rejected the fixture: {events}')
        if process.poll() is not None and not predicate():
            raise RuntimeError(f'Renderer exited early: {process.returncode}')


try:
    send(hello)
    wait_for(lambda: any(e.get('type') == 'ready' for e in events))
    ready = next(e for e in events if e.get('type') == 'ready')
    assert ready['protocol'] == 2, ready
    assert set(ready['capabilities']) >= set(hello['requiredCapabilities']), ready
    cases = (['valid-full-canvas.json', 'valid-fractional-crop.json',
              'valid-reordered-survivor.json', 'clear-canvas-blank.json', 'clear-canvas-omitted.json']
             if args.canvas else ['valid-tiles-text-player-ui.json', 'valid-full-viewport.json',
                                  'clear-empty.json', 'valid-tiles-text-player-ui.json'])
    if args.glyphs:
        cases = ['valid-default.json', 'valid-default.json', 'valid-omitted.json', 'valid-clear.json']
    for number, name in enumerate(cases, 1):
        frame = json.loads((fixtures / name).read_text())
        if args.glyphs and number == 2:
            frame['canvas']['textLayers'][0]['origin']['y'] -= 20
            frame['canvas']['textLayers'][0]['opacity'] = 0.5
            frame['canvas']['textLayers'][0]['clipRect'] = {'x': 35, 'y': 45, 'width': 180, 'height': 92}
        if args.canvas and number == 1:
            # Compose the already validated fixture ingredients into one observable scene.
            acting = json.loads((fixtures / 'valid-selected-acting.json').read_text())
            feedback = json.loads((fixtures / 'valid-transparent-feedback.json').read_text())
            cropped = json.loads((fixtures / 'valid-fractional-crop.json').read_text())['canvas']['images'][0]
            cropped['id'] = 'cropped-fixture'
            cropped['destination']['x'] = 729.5
            frame['canvas']['images'].append(cropped)
            frame['canvas']['indicators'] = acting['canvas']['indicators']
            # Keep the acting base visually separate when the same image is selected.
            frame['canvas']['indicators'][1]['bounds'] = {'x': 989, 'y': 456, 'width': 103, 'height': 2}
            frame['canvas']['textLayers'] = feedback['canvas']['textLayers']
        if args.canvas and name == 'clear-canvas-omitted.json':
            frame['textLayers'] = [{'id': 'legacy-return', 'layer': 0, 'runs': [
                {'row': 1, 'column': 1, 'text': 'Legacy return', 'foreground': None, 'background': None}]}]
        frame['frame'] = number
        send(frame)
        wait_for(lambda: any(d.get('diagnostic') == 'frame'
                             and d.get('frame') == number
                             and d.get('stage') == 'paint_end' for d in diagnostics))
        if (number == 1 or args.glyphs) and args.observe_seconds:
            print(f'Frame {number} painted; silent observation window is open.', flush=True)
            until = time.monotonic() + args.observe_seconds
            while time.monotonic() < until and process.poll() is None:
                drain()
        if args.canvas and number == len(cases) and args.observe_seconds:
            print('Legacy return painted; closing automatically in ten seconds.', flush=True)
            until = time.monotonic() + 10
            while time.monotonic() < until and process.poll() is None:
                drain()
    send({'protocol': 2, 'type': 'shutdown'})
    process.stdin.close()
    wait_for(lambda: process.poll() is not None)
    while selector.get_map():
        drain()
    assert process.returncode == 0, process.returncode
    assert not any(event.get('type') == 'error' for event in events), events
    assert not any(pending.values()), 'Incomplete JSON output'
    assert diagnostics[-1] == {'diagnostic': 'summary', 'dropped_records': 0}
    resources = [d for d in diagnostics if d.get('diagnostic') == 'frame_resources']
    assert [d['tile_cells'] for d in resources] == ([0]*4 if args.glyphs else [0]*5 if args.canvas else [2, 4860, 0, 2]), resources
    if args.canvas:
        assert [d['canvas_images'] for d in resources] == [3, 1, 1, 0, 0], resources
        assert [d['canvas_indicators'] for d in resources] == [2, 1, 0, 0, 0], resources
        assert [d['canvas_text_layers'] for d in resources] == [2, 1, 1, 0, 0], resources
    if args.glyphs:
        glyphs = [dict(d['values']) for d in diagnostics if d.get('diagnostic') == 'geometry' and d.get('stage') == 'glyph_rasters']
        assert [d['builds'] for d in glyphs] == [1, 0, 0, 0], glyphs
        assert [d['canvas_text_layers'] for d in resources] == [1, 1, 1, 0], resources
    receipt = {'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
               'ready': ready, 'frames_painted': len(cases),
               'tile_cells': [d['tile_cells'] for d in resources],
               'canvas_images': [d.get('canvas_images', 0) for d in resources],
               'exit_code': process.returncode, 'silent': True,
               'measurement': 'CPU paint callback completion; not GPU completion or FPS'}
    (args.evidence_dir / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))
finally:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)
    for name, data in raw.items():
        (args.evidence_dir / f'{name}.ndjson').write_bytes(data)
    (args.evidence_dir / 'input.ndjson').write_bytes(b''.join(inputs))
    selector.close()
