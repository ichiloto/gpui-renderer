"""Summarize one retained Engine/GPUI session; CPU stages are not visible FPS."""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import statistics

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('trace', type=Path)
p.add_argument('--pid', type=int, required=True)
p.add_argument('--out', type=Path, required=True)
p.add_argument('--after-probe-start', action='store_true', help='Exclude startup work and graphical frames queued before probe.start')
a = p.parse_args()
raw = a.trace.read_bytes()
source_records = [json.loads(line) for line in raw.splitlines()]
engine = [r for r in source_records if r.get('pid') == a.pid]
# Engine drop summaries omit PID/timestamp. Conservatively retain every unscoped
# summary; filtering by PID first would silently lose evidence of missing data.
engine_drops = [r for r in source_records if r.get('stage') == 'trace.dropped' and r.get('pid') in (None, a.pid)]
assert engine, 'No records for requested Engine PID'
stderr = ''.join(r['text'] for r in engine if r['stage'] == 'renderer.stderr')
native, ordinary = [], []
lines = stderr.split('\n')
trailing = lines.pop()
for line in lines:
    try:
        r = json.loads(line)
    except json.JSONDecodeError:
        if line.startswith('{"diagnostic":'):
            raise
        ordinary.append(line)
        continue
    native.append(r)
if a.after_probe_start:
    starts = [r for r in engine if r['stage'] == 'probe.start']
    assert len(starts) == 1, 'Expected exactly one probe.start'
    after = starts[0]['at_ns']
    queued = [r for r in engine if r['stage'] == 'transport.queued' and r.get('frame') is not None]
    assert len({r['frame'] for r in queued}) == len(queued), 'Source frame IDs repeat; cannot use this workload selector'
    included = {r['frame'] for r in queued if r['at_ns'] >= after}
    engine = [r for r in engine if r['at_ns'] >= after]
    native = [r for r in native if r.get('diagnostic') != 'frame' or r['frame'] in included]
anchors = [r for r in native if r.get('diagnostic') == 'clock_anchor']
assert len([r for r in anchors if r['stage'] == 'start']) <= 1, 'Multiple renderer sessions'
assert all(r['clock'] == 'CLOCK_UPTIME_RAW' for r in anchors)
bounds = None
if anchors:
    lo = max(r['host_ns'] - r['elapsed_after_ns'] for r in anchors)
    hi = min(r['host_ns'] - r['elapsed_before_ns'] for r in anchors)
    assert lo <= hi, 'Inconsistent host clock anchors'
    bounds = [lo, hi]

def dist(values):
    values = sorted(values)
    return {'n': len(values), 'median_ms': statistics.median(values),
            'p95_ms': values[max(0, (95 * len(values) + 99) // 100 - 1)],
            'max_ms': max(values)} if values else None

frames = defaultdict(list)
for r in native:
    if r.get('diagnostic') == 'frame':
        frames[r['sequence']].append(r)
for rs in frames.values():
    rs.sort(key=lambda r: r['at_ns'])
durations, frame_rows, incomplete_cycles = defaultdict(list), [], []
has_paint_observations = any(r.get('diagnostic') == 'frame' and r['stage'] == 'paint_end' for r in native)
for sequence, rs in sorted(frames.items()):
    stages = defaultdict(list)
    for r in rs:
        stages[r['stage']].append(r['at_ns'])
    row = {'sequence': sequence, 'frame': rs[0]['frame'],
           'stage_counts': dict(Counter(r['stage'] for r in rs))}
    for start, end in [('received', 'accepted'), ('accepted', 'replaced'),
                       ('submit_begin', 'submit_end'), ('dequeued', 'replaced')]:
        assert len(stages[start]) <= 1 and len(stages[end]) <= 1, (sequence, start, end)
        if stages[start] and stages[end]:
            ms = (stages[end][0] - stages[start][0]) / 1e6
            assert ms >= 0
            row[start + '_to_' + end + '_ms'] = ms
            durations[start + '_to_' + end].append(ms)
    if stages['replaced'] and stages['render_callback']:
        ms = (stages['render_callback'][0] - stages['replaced'][0]) / 1e6
        assert ms >= 0
        row['replaced_to_first_callback_ms'] = ms
        durations['replaced_to_first_callback'].append(ms)
    # Pair within each callback, never merge first and last stages of repaints.
    cycle = None
    for r in rs:
        if r['stage'] == 'render_callback':
            if cycle is not None and has_paint_observations:
                incomplete_cycles.append(sequence)
            cycle = {}
        if cycle is None:
            continue
        assert r['stage'] not in cycle, (sequence, r['stage'])
        cycle[r['stage']] = r['at_ns']
        for start, end in [('render_callback', 'elements_built'),
                           ('layout_request_begin', 'layout_request_end'),
                           ('layout_request_end', 'prepaint_begin'),
                           ('prepaint_begin', 'prepaint_end'),
                           ('paint_begin', 'paint_end'), ('render_callback', 'paint_end')]:
            if r['stage'] == end and start in cycle:
                ms = (cycle[end] - cycle[start]) / 1e6
                assert ms >= 0
                durations[start + '_to_' + end].append(ms)
        if r['stage'] == 'paint_end':
            cycle = None
    if cycle is not None and has_paint_observations:
        incomplete_cycles.append(sequence)
    frame_rows.append(row)

engine_durations = defaultdict(list)
phase_by_iteration = {r['iteration']: r['phase'] for r in engine if r['stage'] == 'probe.phase'}
phase_durations = defaultdict(lambda: defaultdict(list))
for r in engine:
    if 'duration_ns' in r:
        engine_durations[r['stage']].append(r['duration_ns'] / 1e6)
        phase = phase_by_iteration.get(r['iteration'], 'startup_or_shutdown')
        phase_durations[phase][r['stage']].append(r['duration_ns'] / 1e6)
summary = dict(source=str(a.trace), source_sha256=hashlib.sha256(raw).hexdigest(),
               measurement_scope='after probe.start, frames queued during workload' if a.after_probe_start else 'complete session',
               engine_pid=a.pid, host_offset_bounds_ns=bounds, anchors=anchors,
               boundary='CPU observations only; callback/paint end do not establish GPU completion or visible FPS',
               renderer_dropped=[r['dropped_records'] for r in native if r.get('diagnostic') == 'summary'],
               engine_dropped=engine_drops,
               trailing_stderr=trailing, ordinary_stderr=ordinary,
               geometry=[r for r in native if r.get('diagnostic') == 'geometry'],
               frames=len(frames), renderer_stages=dict(Counter(r['stage'] for r in native if r.get('diagnostic') == 'frame')),
               incomplete_callback_sequences=incomplete_cycles,
               has_paint_observations=has_paint_observations,
               renderer_durations={k: dist(v) for k, v in durations.items()},
               engine_durations={k: dist(v) for k, v in engine_durations.items()},
               phase_durations={phase: {k: dist(v) for k, v in ds.items()} for phase, ds in phase_durations.items()},
               frame_rows=frame_rows)
a.out.mkdir(parents=True, exist_ok=True)
(a.out / 'renderer.ndjson').write_text(''.join(json.dumps(r) + '\n' for r in native))
(a.out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps({k: v for k, v in summary.items() if k not in ('frame_rows', 'geometry', 'anchors', 'phase_durations')}, indent=2))
