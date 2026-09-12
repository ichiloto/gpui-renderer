#!/usr/bin/env python3
"""Summarize renderer callback-to-flush timings; never infer end-to-end game latency.

Example: analyze-trace.py trace.ndjson --events events.ndjson --ids 3:12
Records are correlated by ID and timestamp, not stderr line order: the writer can
receive a key before the producer submits its diagnostic queued record.
"""
import argparse
import json
from pathlib import Path
import statistics

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('trace', type=Path)
parser.add_argument('--events', type=Path)
parser.add_argument('--ids', help='inclusive FIRST:LAST key ID range')
parser.add_argument('--held', action='store_true', help='only GPUI is_held=true events (not reliable for macOS special keys)')
args = parser.parse_args()
records = [json.loads(line) for line in args.trace.read_text().splitlines()]
keys = {}
for record in records:
    if record.get('diagnostic') == 'key':
        stages = keys.setdefault(record['id'], {})
        assert record['stage'] not in stages, ('duplicate stage', record)
        stages[record['stage']] = record

stages = ['native', 'normalized', 'queued', 'write_started', 'written']
complete, ignored, incomplete = {}, [], []
for key_id, key in keys.items():
    if 'ignored' in key:
        ignored.append(key_id)
    elif all(stage in key for stage in stages):
        times = [key[stage]['at_ns'] for stage in stages]
        assert times == sorted(times), ('nonmonotonic key', key_id)
        assert len({key[stage]['is_held'] for stage in stages}) == 1
        complete[key_id] = key
    else:
        incomplete.append(key_id)

if args.events:
    events = [json.loads(line) for line in args.events.read_text().splitlines()]
    assert all(event['type'] in ('ready', 'key', 'close_requested', 'error') for event in events)
    assert len({event['protocol'] for event in events}) <= 1, 'protocol changed in session'
    actual = [event for event in events if event['type'] == 'key']
    expected = [complete[key_id]['written']['key'] for key_id in sorted(complete)]
    assert [event['key'] for event in actual] == expected, 'stdout key/trace mismatch'
    assert all(set(event) == {'protocol', 'type', 'key'} for event in actual), 'diagnostics leaked into stdout'

selected = complete
if args.ids:
    first, last = map(int, args.ids.split(':'))
    assert first <= last
    selected = {i: k for i, k in selected.items() if first <= i <= last}
if args.held:
    selected = {i: k for i, k in selected.items() if k['native']['is_held']}


def distribution(values):
    return dict(median=statistics.median(values), maximum=max(values)) if values else None


def elapsed(start, end):
    return [(key[end]['at_ns'] - key[start]['at_ns']) / 1e6 for key in selected.values()]


def cadence(stage):
    times = sorted(key[stage]['at_ns'] for key in selected.values())
    intervals = [(b-a)/1e6 for a, b in zip(times, times[1:])]
    return dict(interval_ms=distribution(intervals), events_per_second=(len(times)-1)*1e9/(times[-1]-times[0]) if len(times)>1 and times[-1]>times[0] else None)


# These are event-lifetime intervals, not same-instant channel lengths. queued is
# timestamped immediately BEFORE the successful try_send attempt; write_started
# is AFTER dequeue. Thus peak outstanding includes submission and in-flight work.
timeline = []
for key in selected.values():
    timeline.extend([(key['queued']['at_ns'], 1), (key['written']['at_ns'], -1)])
outstanding = peak = 0
for _, change in sorted(timeline, key=lambda item: (item[0], -item[1])):
    outstanding += change
    peak = max(peak, outstanding)

print(json.dumps(dict(
    source=args.trace.name,
    boundary='GPUI on_key_down callback -> successful stdout write_all + flush',
    selected_ids=sorted(selected),
    count=len(selected),
    gpui_is_held_true_count=sum(k['native']['is_held'] for k in selected.values()),
    ignored_ids=ignored,
    incomplete_ids=incomplete,
    dropped_records=[r['dropped_records'] for r in records if r.get('diagnostic')=='summary'],
    callback_to_flush_ms=distribution(elapsed('native','written')),
    normalization_ms=distribution(elapsed('native','normalized')),
    submission_to_writer_start_ms=distribution(elapsed('queued','write_started')),
    write_and_flush_ms=distribution(elapsed('write_started','written')),
    maximum_pending_before_enqueue=max((k['queued']['pending_before_enqueue'] for k in selected.values()), default=0),
    maximum_pending_after_dequeue=max((k['write_started']['pending_after_dequeue'] for k in selected.values()), default=0),
    peak_submission_through_flush_outstanding=peak,
    native_arrival=cadence('native'),
    writer_completion=cadence('written'),
), indent=2))
