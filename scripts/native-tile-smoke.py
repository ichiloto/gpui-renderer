#!/usr/bin/env python3
"""Check tile-capable startup and painting in one brief, silent native window.

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
args = parser.parse_args()
binary = args.binary.resolve(strict=True)
args.evidence_dir.mkdir(parents=True, exist_ok=True)
fixtures = root / 'fixtures/tile-batches'
hello = json.loads((fixtures / 'hello.json').read_text())
hello['assetRoot'] = str(root / 'fixtures')
hello['title'] = 'Ichiloto — renderer startup check (silent)'
events, diagnostics = [], []
raw = {'stdout': bytearray(), 'stderr': bytearray()}
pending = {'stdout': bytearray(), 'stderr': bytearray()}
inputs = []
deadline = time.monotonic() + 20
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
            raise TimeoutError('Native tile check exceeded its 20-second deadline')
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
    cases = ['valid-tiles-text-player-ui.json', 'valid-full-viewport.json',
             'clear-empty.json', 'valid-tiles-text-player-ui.json']
    for number, name in enumerate(cases, 1):
        frame = json.loads((fixtures / name).read_text())
        frame['frame'] = number
        send(frame)
        wait_for(lambda: any(d.get('diagnostic') == 'frame'
                             and d.get('frame') == number
                             and d.get('stage') == 'paint_end' for d in diagnostics))
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
    assert [d['tile_cells'] for d in resources] == [2, 4860, 0, 2], resources
    receipt = {'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
               'ready': ready, 'frames_painted': len(cases),
               'tile_cells': [d['tile_cells'] for d in resources],
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
