#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once __DIR__ . '/lib/Process.php';

runCli(function (): void {
    $args = parseOptions(['binary' => dirname(__DIR__) . '/target/debug/gpui-renderer', 'evidence-dir' => null, 'trace' => false], ['trace'],
        'Real-process IPC checks using silent fixtures. Requires a graphical desktop. Usage: php scripts/native-smoke.php [--binary PATH] [--evidence-dir DIR] [--trace]');
    requireCondition($args['_'] === [], 'Unexpected argument');
    $environment = array_merge(getenv(), ['ICHILOTO_GPUI_TRACE' => $args['trace'] ? '1' : '0']);
    $fixture = implode('', array_map(static fn($message) => encodeJson($message) . "\n", getFixtureMessages()));
    $shutdown = "{\"protocol\":1,\"type\":\"shutdown\"}\n";
    $run = static function ($name, $data, $status, $kinds, ?array $protocols = null) use ($args, $environment): void {
        $process = new Process([$args['binary']], $data, $environment);
        try {
            $code = $process->wait(Process::getTime() + 10);
            $out = $process->output['stdout']; $err = $process->output['stderr'];
            $decode = static fn($text) => array_map(static fn($line) => json_decode($line, true, 512, JSON_THROW_ON_ERROR), $text === '' ? [] : explode("\n", rtrim($text, "\n")));
            $events = $decode($out);
            requireCondition($code === $status, "{$name}: unexpected exit {$code}: {$err}");
            requireCondition(array_column($events, 'type') === $kinds, "{$name}: unexpected events");
            requireCondition(array_column($events, 'protocol') === ($protocols ?? array_fill(0, count($events), 1)), "{$name}: unexpected protocol");
            requireCondition($out === '' || str_ends_with($out, "\n"), 'Incomplete stdout record');
            $errors = array_column(array_filter($events, static fn($event) => $event['type'] === 'error'), 'message');
            $ordinary = $diagnostics = [];
            foreach ($err === '' ? [] : explode("\n", rtrim($err, "\n")) as $line) {
                try { $record = json_decode($line, true, 512, JSON_THROW_ON_ERROR); }
                catch (\JsonException) { $ordinary[] = $line; continue; }
                requireCondition(is_array($record) && isset($record['diagnostic']), "{$name}: invalid diagnostic");
                $diagnostics[] = $record;
            }
            requireCondition($ordinary === $errors, "{$name}: stderr/error mismatch");
            if ($args['trace']) {
                requireCondition(end($diagnostics) === ['diagnostic' => 'summary', 'dropped_records' => 0], 'Missing clean summary');
                requireCondition(array_intersect(array_column($diagnostics, 'diagnostic'), ['clock_anchor', 'clock_anchor_unavailable']) !== [], 'Missing clock anchor');
            } else { requireCondition($diagnostics === [], 'Unexpected diagnostics'); }
            if ($args['evidence-dir'] !== null) {
                makeDirectory($args['evidence-dir']); $stem = $args['evidence-dir'] . '/' . str_replace(' ', '-', $name);
                writeFile($stem . '.stdout.ndjson', $out); writeFile($stem . '.stderr.log', $err); writeFile($stem . '.exit', $code . "\n");
            }
            echo "PASS {$name}: status={$code}; events=", implode(',', $kinds), "\n";
        } finally { $process->stop(); }
    };
    $run('fixture then shutdown drains ready', $fixture . $shutdown, 0, ['ready']);
    $run('recoverable error drains before shutdown', $fixture . "malformed\n" . $shutdown, 0, ['ready', 'error']);
    $run('shutdown before hello', $shutdown, 0, []);
    $run('EOF before hello is fatal', '', 1, ['error']);
    $run('unsupported version then shutdown', "{\"protocol\":3,\"type\":\"shutdown\"}\n" . $shutdown, 0, ['error']);
    foreach (['closed' => $fixture, 'unread' => $fixture . str_repeat("malformed\n", 50000)] as $mode => $input) {
        $process = new Process([$args['binary']], $input, $environment, $mode);
        try { requireCondition($process->wait(Process::getTime() + 10) === 1, "{$mode} stdout must exit with status1"); }
        finally { $process->stop(); }
        echo 'PASS ', $mode === 'closed' ? 'broken stdout: status=1' : 'unread stdout: bounded shutdown, status=1', "\n";
    }
    [$hello, $first, $second] = array_map(static fn($message) => encodeJson($message) . "\n", getFixtureMessages(2));
    $v2 = $hello . $first . $second; $shutdown2 = "{\"protocol\":2,\"type\":\"shutdown\"}\n";
    $run('v2 fixture and style replacement shutdown', $v2 . $shutdown2, 0, ['ready'], [2]);
    $run('v2 malformed and mixed input stays v2', $v2 . "malformed\n" . $shutdown . explode("\n", $fixture)[1] . "\n" . $hello . $shutdown2, 0, ['ready', 'error', 'error', 'error', 'error'], [2, 2, 2, 2, 2]);
    $bad = json_decode($hello, true, 512, JSON_THROW_ON_ERROR); $bad['assetRoot'] = '/nonexistent-ichiloto-assets';
    $run('failed v2 hello then valid v2 session', encodeJson($bad) . "\n" . $v2 . "malformed\n" . $shutdown2, 0, ['error', 'ready', 'error'], [1, 2, 2]);
    $run('v1 rejects mixed v2 without shutdown', $fixture . $shutdown2 . $first . $shutdown, 0, ['ready', 'error', 'error']);
    $bad = json_decode($second, true, 512, JSON_THROW_ON_ERROR); $bad['textLayers'][1]['runs'][0]['background'] = ['kind' => 'ansi16', 'index' => 16];
    $run('v2 invalid colour then valid replacement', $hello . $first . encodeJson($bad) . "\n" . $second . $shutdown2, 0, ['ready', 'error'], [2, 2]);
    $run('v2 invalid run then empty replacement', $hello . $first . '{"protocol":2,"type":"frame","frame":3,"textLayers":[{"id":"bad","layer":0,"runs":[{"row":99,"column":0,"text":"x","foreground":null,"background":null}]}],"sprites":[]}' . "\n" . '{"protocol":2,"type":"frame","frame":4,"textLayers":[],"sprites":[]}' . "\n" . $shutdown2, 0, ['ready', 'error'], [2, 2]);
});
