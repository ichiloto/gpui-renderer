<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once __DIR__ . '/Support.php';

/** Retained burst-evidence entrypoints share one implementation; capture data stays immutable. */
function analyzeBurst(string $defaultBase, int $defaultPid): void
{
    $args = parseOptions(['base' => $defaultBase, 'pid' => (string)$defaultPid, 'out' => null], [],
        'Analyze retained Engine/native burst records. Usage: php analyze.php [--base SESSION_DIR] [--pid PID] [--out DIR]');
    requireCondition(ctype_digit($args['pid']) && $args['_'] === [], 'Expected numeric --pid');
    $pid = (int)$args['pid']; $base = $args['base']; $out = $args['out'] ?? $base . '/evidence';
    $keys = $native = $selected = []; $buffer = '';
    foreach (explode("\n", readFile($base . '/runtime/logs/latency.ndjson')) as $line) {
        try { $r = json_decode($line, true, 512, JSON_THROW_ON_ERROR); } catch (\JsonException) { continue; }
        if (($r['pid'] ?? null) !== $pid) { continue; }
        $stage = $r['stage']; $id = $r['input_id'] ?? null;
        if ($id !== null) { $keys[$id][$stage] ??= $r; }
        if ($stage === 'renderer.stderr') {
            $buffer .= $r['text'];
            while (($end = strpos($buffer, "\n")) !== false) {
                $text = substr($buffer, 0, $end); $buffer = substr($buffer, $end + 1);
                try { $native[] = json_decode($text, true, 512, JSON_THROW_ON_ERROR); }
                catch (\JsonException) { $native[] = ['ordinary_error' => $text]; }
            }
        } elseif ($id !== null || $stage === 'transport.queued') { $selected[] = $r; }
    }
    $anchors = array_values(array_filter($native, static fn($r) => ($r['diagnostic'] ?? '') === 'clock_anchor'));
    requireCondition($anchors !== [], 'No clock anchors');
    $low = max(array_map(static fn($a) => $a['host_ns'] - $a['elapsed_after_ns'], $anchors));
    $high = min(array_map(static fn($a) => $a['host_ns'] - $a['elapsed_before_ns'], $anchors));
    $offset = intdiv($low + $high, 2); $nk = $nf = [];
    foreach ($native as $r) {
        if (($r['diagnostic'] ?? '') === 'key') { $nk[$r['id']][$r['stage']] ??= $r; }
        if (($r['diagnostic'] ?? '') === 'frame') { $nf[$r['sequence']][$r['stage']] ??= $r; }
    }
    $written = array_values(array_filter($nk, static fn($k) => isset($k['written'])));
    $parsed = array_values(array_filter($keys, static fn($k) => isset($k['transport.key.parsed'])));
    requireCondition(count($written) === count($parsed), 'Written/parsed key count mismatch');
    $rows = []; $difference = static fn($a, $b) => $a === null || $b === null ? null : ($b - $a) / 1e6;
    foreach ($written as $index => $n) {
        $k = $parsed[$index]; $p = $k['transport.key.parsed'];
        requireCondition($p['key'] === $n['written']['key'], 'Written/parsed key mismatch');
        $frame = $k['presentation.frame.queued']['frame'] ?? null;
        $matches = array_values(array_filter($nf, static fn($f) => $f['received']['frame'] === $frame));
        requireCondition(count($matches) <= 1, 'Ambiguous frame');
        $ts = array_map(static fn($v) => $offset + $v['at_ns'], $matches[0] ?? []);
        $nativeNs = $offset + $n['native']['at_ns'];
        $rows[] = ['native_id' => $n['native']['id'], 'input_id' => $p['input_id'], 'key' => $p['key'], 'frame' => $frame,
            'native_ns' => $nativeNs, 'before' => $k['game.update.begin']['player'] ?? null, 'after' => $k['game.update.end']['player'] ?? null,
            'event_owns_input' => $k['game.update.end']['event_owns_input'] ?? null, 'state' => $k['game.update.end']['scene_state'] ?? null,
            'times_host_ns' => (object)$ts, 'native_to_received_ms' => $difference($nativeNs, $ts['received'] ?? null),
            'received_to_accepted_ms' => $difference($ts['received'] ?? null, $ts['accepted'] ?? null),
            'accepted_to_replaced_ms' => $difference($ts['accepted'] ?? null, $ts['replaced'] ?? null),
            'replaced_to_callback_ms' => $difference($ts['replaced'] ?? null, $ts['render_callback'] ?? null),
            'native_to_callback_ms' => $difference($nativeNs, $ts['render_callback'] ?? null)];
    }
    $summary = ['pid' => $pid, 'anchors' => $anchors, 'offset_bounds' => [$low, $high], 'incomplete_buffer' => strlen($buffer),
        'keys' => count($rows), 'frames' => count($nf), 'rows' => $rows, 'frame_stages' => (object)$nf,
        'errors' => array_values(array_filter($native, static fn($r) => isset($r['ordinary_error']))),
        'closing_summary' => array_values(array_filter($native, static fn($r) => ($r['diagnostic'] ?? '') === 'summary'))];
    makeDirectory($out); writeFile($out . '/live-summary.json', encodeJson($summary, true) . "\n");
    writeFile($out . '/renderer.ndjson', implode('', array_map(static fn($r) => encodeJson($r) . "\n", $native)));
    writeFile($out . '/engine-selected.ndjson', implode('', array_map(static fn($r) => encodeJson($r) . "\n", $selected)));
    echo encodeJson(['keys' => count($rows), 'frames' => count($nf), 'offset_bounds' => [$low, $high], 'last_rows' => array_slice($rows, -4)], true), "\n";
}

function correlateBurst(string $defaultBase): void
{
    $args = parseOptions(['base' => $defaultBase, 'out' => null], [], 'Correlate burst spans. Usage: php correlate.php [--base EVIDENCE_DIR] [--out DIR]');
    requireCondition($args['_'] === [], 'Unexpected argument'); $base = $args['base'];
    $events = readRecords($base . '/renderer.ndjson'); usort($events, static fn($a, $b) => ($a['at_ns'] ?? 0) <=> ($b['at_ns'] ?? 0));
    $ends = ['elements_built' => 'render_callback', 'layout_request_end' => 'layout_request_begin', 'prepaint_end' => 'prepaint_begin',
        'paint_end' => 'paint_begin', 'prepaint_begin' => 'layout_request_end', 'submit_end' => 'submit_begin', 'submit_failed' => 'submit_begin'];
    $active = $spans = [];
    foreach ($events as $e) {
        if (($e['diagnostic'] ?? '') !== 'frame') { continue; }
        $seq = $e['sequence']; $stage = $e['stage'];
        if (isset($ends[$stage])) {
            $begin = $ends[$stage]; $active[$seq][$begin] ??= [];
            if ($active[$seq][$begin] !== []) {
                $b = array_shift($active[$seq][$begin]); $spans[] = ['sequence' => $seq, 'frame' => $e['frame'], 'stage' => $begin . '→' . $stage,
                    'begin_ns' => $b['at_ns'], 'end_ns' => $e['at_ns'], 'duration_ms' => ($e['at_ns'] - $b['at_ns']) / 1e6];
            }
        }
        if (in_array($stage, $ends, true)) { $active[$seq][$stage][] = $e; }
    }
    $summary = readJson($base . '/live-summary.json'); $rows = [];
    foreach ($summary['rows'] as $row) {
        if ($row['native_id'] < 6 || $row['frame'] === null) { continue; }
        $seq = $row['frame']; $st = $summary['frame_stages'][$seq]; $a = $st['accepted']['at_ns']; $b = $st['dequeued']['at_ns']; $overlapping = [];
        foreach ($spans as $s) {
            if (str_starts_with($s['stage'], 'submit')) { continue; }
            $overlap = max(0, min($b, $s['end_ns']) - max($a, $s['begin_ns'])) / 1e6;
            if ($overlap > 0) { $overlapping[] = $s + ['overlap_ms' => $overlap]; }
        }
        $submission = array_values(array_filter($spans, static fn($s) => $s['sequence'] === $seq && $s['stage'] === 'submit_begin→submit_end'));
        requireCondition($submission !== [], 'Missing submission span');
        $rows[] = ['native_id' => $row['native_id'], 'frame' => $seq, 'accepted_to_dequeued_ms' => ($b - $a) / 1e6,
            'submission_ms' => $submission[0]['duration_ms'], 'overlap' => $overlapping];
    }
    $unpaired = [];
    foreach ($active as $seq => $stages) { foreach ($stages as $stage => $values) { if ($values !== [] && $stage !== 'layout_request_end') { $unpaired[] = ['sequence' => $seq, 'stage' => $stage, 'count' => count($values)]; } } }
    $out = $args['out'] ?? $base; makeDirectory($out);
    writeFile($out . '/span-correlation.json', encodeJson(['spans' => $spans, 'rows' => $rows, 'unpaired' => $unpaired], true) . "\n");
    foreach (array_slice($rows, -4) as $row) {
        echo 'key ', $row['native_id'], ' frame ', $row['frame'], ' accepted→dequeue ', round($row['accepted_to_dequeued_ms'], 3),
            ' submit ', round($row['submission_ms'], 3), ' covered ', round(array_sum(array_column($row['overlap'], 'overlap_ms')), 3), "\n";
        foreach ($row['overlap'] as $s) { echo ' previous/current ', $s['sequence'], ' ', $s['stage'], ' ', round($s['overlap_ms'], 3), "\n"; }
    }
    echo 'unpaired ', encodeJson($unpaired), "\n";
}
