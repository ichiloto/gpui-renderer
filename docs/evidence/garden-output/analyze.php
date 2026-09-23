#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once dirname(__DIR__, 3) . '/scripts/lib/Support.php';

runCli(function (): void {
    $args = parseOptions(['pid' => null, 'out' => null, 'after-probe-start' => false], ['after-probe-start'],
        'Summarize a retained Engine/GPUI session; CPU stages are not visible FPS. Usage: php analyze.php TRACE --pid PID --out DIR [--after-probe-start]');
    requireCondition(count($args['_']) === 1 && ctype_digit($args['pid'] ?? '') && $args['out'] !== null, 'Expected trace, --pid and --out');
    $path = $args['_'][0]; $pid = (int)$args['pid']; $source = readRecords($path);
    $engine = array_values(array_filter($source, static fn($r) => ($r['pid'] ?? null) === $pid));
    $drops = array_values(array_filter($source, static fn($r) => ($r['stage'] ?? '') === 'trace.dropped' && in_array($r['pid'] ?? null, [null, $pid], true)));
    requireCondition($engine !== [], 'No records for requested Engine PID');
    $stderr = implode('', array_column(array_filter($engine, static fn($r) => $r['stage'] === 'renderer.stderr'), 'text'));
    $native = $ordinary = []; $lines = explode("\n", $stderr); $trailing = array_pop($lines);
    foreach ($lines as $line) {
        try { $native[] = json_decode($line, true, 512, JSON_THROW_ON_ERROR); }
        catch (\JsonException $error) { if (str_starts_with($line, '{"diagnostic":')) { throw $error; } $ordinary[] = $line; }
    }
    if ($args['after-probe-start']) {
        $starts = array_values(array_filter($engine, static fn($r) => $r['stage'] === 'probe.start'));
        requireCondition(count($starts) === 1, 'Expected exactly one probe.start'); $after = $starts[0]['at_ns'];
        $queued = array_values(array_filter($engine, static fn($r) => $r['stage'] === 'transport.queued' && isset($r['frame'])));
        requireCondition(count(array_unique(array_column($queued, 'frame'))) === count($queued), 'Source frame IDs repeat; cannot use workload selector');
        $included = array_column(array_filter($queued, static fn($r) => $r['at_ns'] >= $after), 'frame');
        $engine = array_values(array_filter($engine, static fn($r) => $r['at_ns'] >= $after));
        $native = array_values(array_filter($native, static fn($r) => ($r['diagnostic'] ?? '') !== 'frame' || in_array($r['frame'], $included, true)));
    }
    $anchors = array_values(array_filter($native, static fn($r) => ($r['diagnostic'] ?? '') === 'clock_anchor'));
    requireCondition(count(array_filter($anchors, static fn($r) => $r['stage'] === 'start')) <= 1, 'Multiple renderer sessions');
    foreach ($anchors as $r) { requireCondition($r['clock'] === 'CLOCK_UPTIME_RAW', 'Unexpected host clock'); }
    $bounds = null;
    if ($anchors !== []) {
        $lo = max(array_map(static fn($r) => $r['host_ns'] - $r['elapsed_after_ns'], $anchors));
        $hi = min(array_map(static fn($r) => $r['host_ns'] - $r['elapsed_before_ns'], $anchors));
        requireCondition($lo <= $hi, 'Inconsistent host clock anchors'); $bounds = [$lo, $hi];
    }
    $frames = $durations = $rows = $incomplete = $counts = [];
    $hasPaint = false;
    foreach ($native as $r) {
        if (($r['diagnostic'] ?? '') !== 'frame') { continue; }
        $frames[$r['sequence']][] = $r;
        $counts[$r['stage']] = ($counts[$r['stage']] ?? 0) + 1;
        $hasPaint = $hasPaint || $r['stage'] === 'paint_end';
    }
    ksort($frames, SORT_NUMERIC);
    $recordDuration = static function ($key, $start, $end) use (&$durations): float {
        $ms = ($end - $start) / 1e6; requireCondition($ms >= 0, 'Negative stage duration'); $durations[$key][] = $ms; return $ms;
    };
    foreach ($frames as $sequence => $rs) {
        usort($rs, static fn($a, $b) => $a['at_ns'] <=> $b['at_ns']);
        $stages = []; foreach ($rs as $r) { $stages[$r['stage']][] = $r['at_ns']; }
        $row = ['sequence' => $sequence, 'frame' => $rs[0]['frame'], 'stage_counts' => (object)array_count_values(array_column($rs, 'stage'))];
        foreach ([['received', 'accepted'], ['accepted', 'replaced'], ['submit_begin', 'submit_end'], ['dequeued', 'replaced']] as [$start, $end]) {
            requireCondition(count($stages[$start] ?? []) <= 1 && count($stages[$end] ?? []) <= 1, 'Repeated unique frame stage');
            if (isset($stages[$start][0], $stages[$end][0])) { $row[$start . '_to_' . $end . '_ms'] = $recordDuration($start . '_to_' . $end, $stages[$start][0], $stages[$end][0]); }
        }
        if (isset($stages['replaced'][0], $stages['render_callback'][0])) { $row['replaced_to_first_callback_ms'] = $recordDuration('replaced_to_first_callback', $stages['replaced'][0], $stages['render_callback'][0]); }
        $cycle = null;
        foreach ($rs as $r) {
            if ($r['stage'] === 'render_callback') { if ($cycle !== null && $hasPaint) { $incomplete[] = $sequence; } $cycle = []; }
            if ($cycle === null) { continue; }
            requireCondition(!isset($cycle[$r['stage']]), 'Duplicate callback stage'); $cycle[$r['stage']] = $r['at_ns'];
            foreach ([['render_callback', 'elements_built'], ['layout_request_begin', 'layout_request_end'], ['layout_request_end', 'prepaint_begin'],
                ['prepaint_begin', 'prepaint_end'], ['paint_begin', 'paint_end'], ['render_callback', 'paint_end']] as [$start, $end]) {
                if ($r['stage'] === $end && isset($cycle[$start])) { $recordDuration($start . '_to_' . $end, $cycle[$start], $cycle[$end]); }
            }
            if ($r['stage'] === 'paint_end') { $cycle = null; }
        }
        if ($cycle !== null && $hasPaint) { $incomplete[] = $sequence; } $rows[] = $row;
    }
    $engineDurations = $phases = $phaseDurations = [];
    foreach ($engine as $r) { if ($r['stage'] === 'probe.phase') { $phases[$r['iteration']] = $r['phase']; } }
    foreach ($engine as $r) {
        if (!isset($r['duration_ns'])) { continue; }
        $engineDurations[$r['stage']][] = $r['duration_ns'] / 1e6;
        $phaseDurations[$phases[$r['iteration']] ?? 'startup_or_shutdown'][$r['stage']][] = $r['duration_ns'] / 1e6;
    }
    $summary = ['source' => $path, 'source_sha256' => hash_file('sha256', $path),
        'measurement_scope' => $args['after-probe-start'] ? 'after probe.start, frames queued during workload' : 'complete session',
        'engine_pid' => $pid, 'host_offset_bounds_ns' => $bounds, 'anchors' => $anchors,
        'boundary' => 'CPU observations only; callback/paint end do not establish GPU completion or visible FPS',
        'renderer_dropped' => array_column(array_filter($native, static fn($r) => ($r['diagnostic'] ?? '') === 'summary'), 'dropped_records'),
        'engine_dropped' => $drops, 'trailing_stderr' => $trailing, 'ordinary_stderr' => $ordinary,
        'geometry' => array_values(array_filter($native, static fn($r) => ($r['diagnostic'] ?? '') === 'geometry')),
        'frames' => count($frames), 'renderer_stages' => (object)$counts, 'incomplete_callback_sequences' => $incomplete,
        'has_paint_observations' => $hasPaint,
        'renderer_durations' => (object)array_map(getMillisecondsDistribution(...), $durations),
        'engine_durations' => (object)array_map(getMillisecondsDistribution(...), $engineDurations),
        'phase_durations' => (object)array_map(static fn($ds) => (object)array_map(getMillisecondsDistribution(...), $ds), $phaseDurations), 'frame_rows' => $rows];
    makeDirectory($args['out']);
    writeFile($args['out'] . '/renderer.ndjson', implode('', array_map(static fn($r) => encodeJson($r) . "\n", $native)));
    writeFile($args['out'] . '/summary.json', encodeJson($summary, true) . "\n");
    echo encodeJson(array_diff_key($summary, array_flip(['frame_rows', 'geometry', 'anchors', 'phase_durations'])), true), "\n";
});
