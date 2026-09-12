#!/usr/bin/env python3
"""One silent native fixture. Commands on stdin: first, second, invalid, clear, shutdown.
Renderer NDJSON stays on stdout and diagnostics on stderr. Native keyboard events
must be generated in the actual window, not through these fixture controls.
"""
import argparse
import copy
import json
from pathlib import Path
import select
import subprocess
import sys

root = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, default=root / 'target/debug/gpui-renderer')
parser.add_argument('--protocol', type=int, choices=[1, 2], default=2)
parser.add_argument('--pid-file', type=Path)
parser.add_argument('--geometry', choices=['original', 'last-legend', 'oversized', 'oversized-tall'], default='original')
args = parser.parse_args()
fixture = 'home-frame.ndjson' if args.protocol == 1 else 'presentation-v2.ndjson'
messages = [json.loads(line) for line in (root / 'fixtures' / fixture).read_text().splitlines()]
messages[0]['assetRoot'] = str(root / 'fixtures')
if args.geometry != 'original':
    columns, rows = 135, 36
    cw, ch = {'last-legend': (10, 20), 'oversized': (20, 40), 'oversized-tall': (20, 80)}[args.geometry]
    messages[0]['grid'] = dict(columns=columns, rows=rows, cellWidth=cw, cellHeight=ch)
    messages[0]['title'] = f'Ichiloto - {args.geometry} viewport (silent)'
    top = ('+' + f' {columns}x{rows} / {cw}x{ch} / TOP / LEFT ' + '-' * columns)[:columns - 1] + '+'
    bottom = ('+' + ' BOTTOM / ALL ROWS AND COLUMNS VISIBLE ' + '-' * columns)[:columns - 1] + '+'
    for frame in messages[1:]:
        if args.protocol == 1:
            frame['text'] += [''] * (rows - len(frame['text']))
            frame['text'][0], frame['text'][-1] = top, bottom
            for row in range(1, rows - 1):
                frame['text'][row] = '|' + frame['text'][row][1:].ljust(columns - 2) + '|'
        else:
            runs = [dict(row=0, column=0, text=top), dict(row=rows-1, column=0, text=bottom)]
            runs += [dict(row=row, column=column, text='|') for row in range(1, rows-1) for column in (0, columns-1)]
            for run in runs:
                run.update(foreground=dict(kind='ansi16', index=14), background=None)
            frame['textLayers'].insert(0, dict(id='viewport-extents', layer=-1, runs=runs))
process = subprocess.Popen([str(args.binary)], stdin=subprocess.PIPE)
if args.pid_file:
    args.pid_file.write_text(str(process.pid) + '\n')


def send(message):
    process.stdin.write((json.dumps(message, separators=(',', ':')) + '\n').encode())
    process.stdin.flush()


try:
    send(messages[0])
    send(messages[1])
    while process.poll() is None:
        ready, _, _ = select.select([sys.stdin], [], [], 0.1)
        if not ready:
            continue
        command = sys.stdin.readline().strip()
        if command in ('shutdown', ''):
            send(dict(protocol=args.protocol, type='shutdown'))
            break
        if command == 'first':
            send(messages[1])
        elif command == 'second' and args.protocol == 2:
            send(messages[2])
        elif command == 'invalid':
            invalid = copy.deepcopy(messages[1])
            invalid['sprites'][0]['asset'] = 'missing.png'
            send(invalid)
        elif command == 'clear':
            frame = dict(protocol=args.protocol, type='frame', frame=3, sprites=[])
            frame['text' if args.protocol == 1 else 'textLayers'] = []
            send(frame)
    status = process.wait(timeout=5)
finally:
    process.stdin.close()
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=5)
sys.exit(status)
