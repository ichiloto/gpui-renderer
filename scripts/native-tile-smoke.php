#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once __DIR__ . '/lib/Process.php';

runCli(function (): void {
    $args = parseOptions(['binary' => null, 'evidence-dir' => null, 'glyphs' => false, 'canvas' => false, 'observe-seconds' => '0'], ['glyphs', 'canvas'],
        'One silent native fixture window; CPU callbacks are not GPU completion or FPS. Usage: php scripts/native-tile-smoke.php --binary PATH --evidence-dir DIR [--canvas|--glyphs] [--observe-seconds 0..45]');
    requireCondition($args['_'] === [] && $args['binary'] !== null && $args['evidence-dir'] !== null, '--binary and --evidence-dir are required');
    requireCondition(is_numeric($args['observe-seconds']) && is_finite((float)$args['observe-seconds']) && (float)$args['observe-seconds'] >= 0 && (float)$args['observe-seconds'] <= 45, '--observe-seconds must be in 0..45');
    requireCondition(!$args['canvas'] || !$args['glyphs'], '--canvas and --glyphs are separate scenarios');
    $observe = (float)$args['observe-seconds'];
    $binary = realpath($args['binary']); requireCondition($binary !== false && is_file($binary), 'Renderer binary does not exist');
    makeDirectory($args['evidence-dir']);
    $root = dirname(__DIR__);
    $fixtures = $root . '/fixtures/' . ($args['glyphs'] ? 'canvas-glyph-effects' : ($args['canvas'] ? 'graphical-canvas' : 'tile-batches'));
    $hello = readJson($fixtures . '/hello.json'); $hello['assetRoot'] = $root . '/fixtures';
    $hello['title'] = 'Ichiloto — renderer startup check (silent)';
    $events = $diagnostics = $inputs = [];
    $pending = ['stdout' => '', 'stderr' => '']; $positions = ['stdout' => 0, 'stderr' => 0];
    $deadline = Process::getTime() + 30 + $observe * ($args['glyphs'] ? 4 : 1);
    $process = new Process([$binary], environment: array_merge(getenv(), ['ICHILOTO_GPUI_TRACE' => '1']));
    $send = static function (array $message) use ($process, &$inputs, $deadline): void {
        $data = encodeJson($message) . "\n"; $inputs[] = $data; $process->send($data, $deadline);
    };
    $drain = static function () use ($process, &$positions, &$pending, &$events, &$diagnostics): void {
        $process->drain();
        foreach (array_keys($positions) as $name) {
            $pending[$name] .= substr($process->output[$name], $positions[$name]);
            $positions[$name] = strlen($process->output[$name]);
            while (($end = strpos($pending[$name], "\n")) !== false) {
                $line = substr($pending[$name], 0, $end); $pending[$name] = substr($pending[$name], $end + 1);
                $record = json_decode($line, true, 512, JSON_THROW_ON_ERROR);
                if ($name === 'stdout') { $events[] = $record; } else { $diagnostics[] = $record; }
            }
        }
    };
    $wait = static function (callable $predicate) use ($process, $deadline, $drain, &$events): void {
        while (!$predicate()) {
            requireCondition(Process::getTime() < $deadline, 'Native check exceeded bounded deadline');
            $drain();
            requireCondition(array_filter($events, static fn($event) => ($event['type'] ?? '') === 'error') === [], 'Renderer rejected fixture');
            requireCondition($process->isRunning() || $predicate(), 'Renderer exited early');
            usleep(5000);
        }
    };
    $hold = static function (float $seconds) use ($process, $drain): void {
        $until = Process::getTime() + $seconds;
        while (Process::getTime() < $until && $process->isRunning()) { $drain(); usleep(10000); }
    };
    try {
        $send($hello);
        $wait(static function () use (&$events): bool { return in_array('ready', array_column($events, 'type'), true); });
        $ready = array_values(array_filter($events, static fn($event) => ($event['type'] ?? '') === 'ready'))[0];
        requireCondition($ready['protocol'] === 2 && array_diff($hello['requiredCapabilities'], $ready['capabilities']) === [], 'Ready lacks requested capabilities');
        $cases = $args['canvas'] ? ['valid-full-canvas.json', 'valid-fractional-crop.json', 'valid-reordered-survivor.json', 'clear-canvas-blank.json', 'clear-canvas-omitted.json']
            : ['valid-tiles-text-player-ui.json', 'valid-full-viewport.json', 'clear-empty.json', 'valid-tiles-text-player-ui.json'];
        if ($args['glyphs']) { $cases = ['valid-default.json', 'valid-default.json', 'valid-omitted.json', 'valid-clear.json']; }
        foreach ($cases as $index => $name) {
            $number = $index + 1; $frame = readJson($fixtures . '/' . $name);
            if ($args['glyphs'] && $number === 2) {
                $frame['canvas']['textLayers'][0]['origin']['y'] -= 20;
                $frame['canvas']['textLayers'][0]['opacity'] = 0.5;
                $frame['canvas']['textLayers'][0]['clipRect'] = ['x' => 35, 'y' => 45, 'width' => 180, 'height' => 92];
            }
            if ($args['canvas'] && $number === 1) {
                $acting = readJson($fixtures . '/valid-selected-acting.json');
                $feedback = readJson($fixtures . '/valid-transparent-feedback.json');
                $cropped = readJson($fixtures . '/valid-fractional-crop.json')['canvas']['images'][0];
                $cropped['id'] = 'cropped-fixture'; $cropped['destination']['x'] = 729.5;
                $frame['canvas']['images'][] = $cropped; $frame['canvas']['indicators'] = $acting['canvas']['indicators'];
                $frame['canvas']['indicators'][1]['bounds'] = ['x' => 989, 'y' => 456, 'width' => 103, 'height' => 2];
                $frame['canvas']['textLayers'] = $feedback['canvas']['textLayers'];
            }
            if ($args['canvas'] && $name === 'clear-canvas-omitted.json') {
                $frame['textLayers'] = [['id' => 'legacy-return', 'layer' => 0, 'runs' => [
                    ['row' => 1, 'column' => 1, 'text' => 'Legacy return', 'foreground' => null, 'background' => null]]]];
            }
            $frame['frame'] = $number; $send($frame);
            $wait(static function () use (&$diagnostics, $number): bool {
                return array_filter($diagnostics, static fn($d) => ($d['diagnostic'] ?? '') === 'frame'
                    && ($d['frame'] ?? null) === $number && ($d['stage'] ?? '') === 'paint_end') !== [];
            });
            if (($number === 1 || $args['glyphs']) && $observe > 0) { echo "Frame {$number} painted; silent observation window is open.\n"; $hold($observe); }
            if ($args['canvas'] && $number === count($cases) && $observe > 0) { echo "Legacy return painted; closing automatically in ten seconds.\n"; $hold(10); }
        }
        $send(['protocol' => 2, 'type' => 'shutdown']); $process->closeInput();
        $wait(static fn() => !$process->isRunning()); $drain();
        requireCondition($process->exitCode === 0, 'Renderer exited unsuccessfully');
        requireCondition(array_filter($events, static fn($event) => ($event['type'] ?? '') === 'error') === [], 'Renderer reported an error');
        requireCondition($pending === ['stdout' => '', 'stderr' => ''], 'Incomplete JSON output');
        requireCondition(end($diagnostics) === ['diagnostic' => 'summary', 'dropped_records' => 0], 'Missing clean summary');
        $resources = array_values(array_filter($diagnostics, static fn($d) => ($d['diagnostic'] ?? '') === 'frame_resources'));
        requireCondition(array_column($resources, 'tile_cells') === ($args['glyphs'] ? [0, 0, 0, 0] : ($args['canvas'] ? [0, 0, 0, 0, 0] : [2, 4860, 0, 2])), 'Unexpected tile resources');
        if ($args['canvas']) {
            foreach (['canvas_images' => [3, 1, 1, 0, 0], 'canvas_indicators' => [2, 1, 0, 0, 0], 'canvas_text_layers' => [2, 1, 1, 0, 0]] as $field => $expected) {
                requireCondition(array_column($resources, $field) === $expected, "Unexpected {$field}");
            }
        }
        if ($args['glyphs']) {
            $builds = [];
            foreach ($diagnostics as $d) { if (($d['diagnostic'] ?? '') === 'geometry' && ($d['stage'] ?? '') === 'glyph_rasters') { $builds[] = (int)array_column($d['values'], 1, 0)['builds']; } }
            requireCondition($builds === [1, 0, 0, 0], 'Unexpected glyph builds');
            requireCondition(array_column($resources, 'canvas_text_layers') === [1, 1, 1, 0], 'Unexpected glyph text layers');
        }
        $receipt = ['binary_sha256' => hash_file('sha256', $binary), 'ready' => $ready, 'frames_painted' => count($cases),
            'tile_cells' => array_column($resources, 'tile_cells'),
            'canvas_images' => array_map(static fn($d) => $d['canvas_images'] ?? 0, $resources),
            'exit_code' => $process->exitCode, 'silent' => true, 'measurement' => 'CPU paint callback completion; not GPU completion or FPS'];
        writeFile($args['evidence-dir'] . '/receipt.json', encodeJson($receipt, true) . "\n"); echo encodeJson($receipt, true), "\n";
    } finally {
        $process->stop();
        foreach ($process->output as $name => $data) { writeFile($args['evidence-dir'] . '/' . $name . '.ndjson', $data); }
        writeFile($args['evidence-dir'] . '/input.ndjson', implode('', $inputs));
    }
});
