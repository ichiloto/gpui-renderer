#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once __DIR__ . '/lib/Support.php';

runCli(function (): void {
    $args = parseOptions(['events' => null, 'ids' => null, 'held' => false], ['held'],
        'Summarize callback-to-flush timing, not end-to-end latency. Usage: php scripts/analyze-trace.php TRACE [--events FILE] [--ids FIRST:LAST] [--held]');
    requireCondition(count($args['_']) === 1, 'Expected one trace path');
    $records = $ordinary = $keys = [];
    $text = readFile($args['_'][0]);
    $lines = $text === '' ? [] : preg_split('/\r\n|\n|\r/', $text);
    if ($lines !== [] && end($lines) === '') { array_pop($lines); }
    foreach ($lines as $line) {
        try { $records[] = json_decode($line, true, 512, JSON_THROW_ON_ERROR); }
        catch (\JsonException $error) {
            if (str_starts_with($line, '{"diagnostic":')) { throw $error; }
            $ordinary[] = $line;
        }
    }
    foreach ($records as $record) {
        if (($record['diagnostic'] ?? null) !== 'key') { continue; }
        $id = $record['id']; $stage = $record['stage'];
        requireCondition(!isset($keys[$id][$stage]), 'Duplicate key stage');
        $keys[$id][$stage] = $record;
    }
    $stages = ['native', 'normalized', 'queued', 'write_started', 'written'];
    $complete = $ignored = $incomplete = [];
    foreach ($keys as $id => $key) {
        if (isset($key['ignored'])) { $ignored[] = $id; }
        elseif (array_diff($stages, array_keys($key)) === []) {
            $times = array_map(static fn($stage) => $key[$stage]['at_ns'], $stages);
            $sorted = $times; sort($sorted, SORT_NUMERIC);
            requireCondition($times === $sorted, "Nonmonotonic key {$id}");
            requireCondition(count(array_unique(array_map(static fn($stage) => $key[$stage]['is_held'], $stages))) === 1, 'Inconsistent held state');
            $complete[$id] = $key;
        } else { $incomplete[] = $id; }
    }
    if ($args['events'] !== null) {
        $events = readRecords($args['events']);
        foreach ($events as $event) { requireCondition(in_array($event['type'], ['ready', 'key', 'close_requested', 'error'], true), 'Unexpected event'); }
        requireCondition(count(array_unique(array_column($events, 'protocol'))) <= 1, 'Protocol changed in session');
        $actual = array_values(array_filter($events, static fn($event) => $event['type'] === 'key'));
        $ordered = $complete; ksort($ordered, SORT_NUMERIC);
        requireCondition(array_column($actual, 'key') === array_values(array_map(static fn($key) => $key['written']['key'], $ordered)), 'stdout key/trace mismatch');
        foreach ($actual as $event) {
            $names = array_keys($event); sort($names);
            requireCondition($names === ['key', 'protocol', 'type'], 'Diagnostics leaked into stdout');
        }
    }
    $selected = $complete;
    if ($args['ids'] !== null) {
        requireCondition(preg_match('/^(\d+):(\d+)$/', $args['ids'], $match) === 1 && (int)$match[1] <= (int)$match[2], 'Expected inclusive FIRST:LAST range');
        $selected = array_filter($selected, static fn($id) => $id >= (int)$match[1] && $id <= (int)$match[2], ARRAY_FILTER_USE_KEY);
    }
    if ($args['held']) { $selected = array_filter($selected, static fn($key) => $key['native']['is_held']); }
    $elapsed = static fn($start, $end) => array_map(static fn($key) => ($key[$end]['at_ns'] - $key[$start]['at_ns']) / 1e6, $selected);
    $cadence = static function ($stage) use ($selected): array {
        $times = array_values(array_map(static fn($key) => $key[$stage]['at_ns'], $selected)); sort($times, SORT_NUMERIC);
        $intervals = [];
        for ($i = 1; $i < count($times); $i++) { $intervals[] = ($times[$i] - $times[$i - 1]) / 1e6; }
        return ['interval_ms' => getDistribution($intervals), 'events_per_second' => count($times) > 1 && end($times) > $times[0]
            ? (count($times) - 1) * 1e9 / (end($times) - $times[0]) : null];
    };
    $timeline = [];
    foreach ($selected as $key) { $timeline[] = [$key['queued']['at_ns'], 1]; $timeline[] = [$key['written']['at_ns'], -1]; }
    usort($timeline, static fn($a, $b) => ($a[0] <=> $b[0]) ?: ($b[1] <=> $a[1]));
    $outstanding = $peak = 0;
    foreach ($timeline as [, $change]) { $outstanding += $change; $peak = max($peak, $outstanding); }
    $ids = array_keys($selected); sort($ids, SORT_NUMERIC);
    echo encodeJson([
        'source' => basename($args['_'][0]), 'boundary' => 'GPUI on_key_down callback -> successful stdout write_all + flush',
        'selected_ids' => $ids, 'count' => count($selected),
        'gpui_is_held_true_count' => count(array_filter($selected, static fn($key) => $key['native']['is_held'])),
        'ignored_ids' => $ignored, 'incomplete_ids' => $incomplete,
        'dropped_records' => array_column(array_filter($records, static fn($r) => ($r['diagnostic'] ?? '') === 'summary'), 'dropped_records'),
        'ordinary_stderr_messages' => $ordinary,
        'callback_to_flush_ms' => getDistribution($elapsed('native', 'written')),
        'normalization_ms' => getDistribution($elapsed('native', 'normalized')),
        'submission_to_writer_start_ms' => getDistribution($elapsed('queued', 'write_started')),
        'write_and_flush_ms' => getDistribution($elapsed('write_started', 'written')),
        'maximum_pending_before_enqueue' => max([0, ...array_map(static fn($key) => $key['queued']['pending_before_enqueue'], array_values($selected))]),
        'maximum_pending_after_dequeue' => max([0, ...array_map(static fn($key) => $key['write_started']['pending_after_dequeue'], array_values($selected))]),
        'peak_submission_through_flush_outstanding' => $peak, 'native_arrival' => $cadence('native'), 'writer_completion' => $cadence('written'),
    ], true), "\n";
});
