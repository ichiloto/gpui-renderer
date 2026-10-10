#!/usr/bin/env php
<?php

/** Protocol test double. No GPUI, window, game, audio, or saves. */
declare(strict_types=1);
$trace = getenv('ICHILOTO_GPUI_TRACE') === '1';
$protocol = 1; $hello = null;
$emit = static function (array $message): void {
    if (@fwrite(STDOUT, json_encode($message, JSON_THROW_ON_ERROR) . "\n") === false) { exit(1); }
    fflush(STDOUT);
};
$diagnose = static function (array $message) use ($trace): void {
    if ($trace) { fwrite(STDERR, json_encode($message, JSON_THROW_ON_ERROR) . "\n"); fflush(STDERR); }
};
$diagnose(['diagnostic' => 'clock_anchor_unavailable']);
$error = static function () use (&$protocol, $emit): void {
    $emit(['protocol' => $protocol, 'type' => 'error', 'message' => 'Fixture rejected']);
    fwrite(STDERR, "Fixture rejected\n");
};
register_shutdown_function(static fn() => $diagnose(['diagnostic' => 'summary', 'dropped_records' => 0]));
// Smoke driver pipe-failure cases are controlled exits, not a claim of native queue/backpressure validation.
if ((fstat(STDOUT)['mode'] & 0170000) === 0010000 && getenv('FAKE_SMOKE') === '1') { exit(1); }
while (($line = fgets(STDIN)) !== false) {
    if ($capture = getenv('FAKE_CAPTURE')) { file_put_contents($capture, $line, FILE_APPEND); }
    try { $message = json_decode($line, true, 512, JSON_THROW_ON_ERROR); }
    catch (JsonException) { $error(); continue; }
    $version = $message['protocol'];
    if (!in_array($version, [1, 2], true) || ($hello !== null && $version !== $protocol)) { $error(); continue; }
    if ($message['type'] === 'shutdown') { exit(0); }
    if ($message['type'] === 'hello') {
        if ($hello !== null || !is_dir($message['assetRoot'])) { $error(); continue; }
        $protocol = $version; $hello = $message;
        $emit(['protocol' => $protocol, 'type' => 'ready', 'capabilities' => $message['requiredCapabilities'] ?? []]);
        continue;
    }
    if ($hello === null) { $error(); continue; }
    $bad = false;
    foreach ($message['textLayers'] ?? [] as $layer) {
        foreach ($layer['runs'] as $run) {
            if ($run['row'] >= $hello['grid']['rows'] || (($run['background']['kind'] ?? '') === 'ansi16' && ($run['background']['index'] ?? 0) > 15)) { $bad = true; }
        }
    }
    foreach ($message['sprites'] ?? [] as $sprite) { if ($sprite['asset'] === 'missing.png') { $bad = true; } }
    if ($bad) { $error(); continue; }
    $frame = $message['frame']; $canvas = $message['canvas'] ?? [];
    $diagnose(['diagnostic' => 'frame_resources', 'frame' => $frame,
        'tile_cells' => array_sum(array_map(static fn($batch) => count($batch['cells']), $message['tileBatches'] ?? [])),
        'canvas_images' => count($canvas['images'] ?? []), 'canvas_indicators' => count($canvas['indicators'] ?? []),
        'canvas_text_layers' => count($canvas['textLayers'] ?? [])]);
    if (in_array('canvas_glyph_effects', $hello['requiredCapabilities'] ?? [], true)) {
        $diagnose(['diagnostic' => 'geometry', 'stage' => 'glyph_rasters', 'values' => [['builds', $frame === 1 ? 1.0 : 0.0]]]);
    }
    $diagnose(['diagnostic' => 'frame', 'frame' => $frame, 'stage' => 'paint_end']);
}
$error(); exit(1);
