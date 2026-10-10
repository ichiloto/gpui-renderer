#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once __DIR__ . '/lib/Process.php';

runCli(function (): void {
    $args = parseOptions(['binary' => null, 'replay' => null, 'evidence-dir' => null,
        'hold-frame' => '1', 'observe-seconds' => '0'], [],
        'Silent native retained replay. Usage: php scripts/native-retained-replay.php --binary PATH --replay NDJSON --evidence-dir DIR [--hold-frame N] [--observe-seconds 0..45]');
    requireCondition($args['_'] === [] && $args['binary'] !== null && $args['replay'] !== null
        && $args['evidence-dir'] !== null, '--binary, --replay and --evidence-dir are required');
    $binary = realpath($args['binary']);
    requireCondition($binary !== false && is_file($binary) && is_executable($binary), 'Renderer binary does not exist');
    requireCondition(ctype_digit($args['hold-frame']) && (int)$args['hold-frame'] > 0, 'hold-frame must be positive');
    requireCondition(is_numeric($args['observe-seconds']) && is_finite((float)$args['observe-seconds'])
        && (float)$args['observe-seconds'] >= 0 && (float)$args['observe-seconds'] <= 45,
        'observe-seconds must be in 0..45');
    $lines = file($args['replay'], FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES);
    requireCondition($lines !== false && count($lines) >= 2, 'Replay must contain hello and retained frames');
    $messages = array_map(static fn(string $line): array => json_decode($line, true, 512, JSON_THROW_ON_ERROR), $lines);
    requireCondition(($messages[0]['protocol'] ?? null) === 2 && ($messages[0]['type'] ?? null) === 'hello',
        'Replay must start with a protocol 2 hello');
    requireCondition(is_dir($messages[0]['assetRoot'] ?? ''), 'Replay asset root does not exist');
    foreach (array_slice($messages, 1) as $message) {
        requireCondition(($message['protocol'] ?? null) === 2 && ($message['type'] ?? null) === 'frame'
            && isset($message['generation'], $message['baseGeneration'], $message['operations']),
            'Replay contains a non-retained frame');
    }
    makeDirectory($args['evidence-dir']);
    $messages[0]['title'] = 'Ichiloto — retained visual check (silent)';
    $process = new Process([$binary], environment: array_merge(getenv(), ['ICHILOTO_GPUI_TRACE' => '1']));
    $events = $diagnostics = $sent = [];
    $positions = ['stdout' => 0, 'stderr' => 0];
    $pending = ['stdout' => '', 'stderr' => ''];
    $deadline = Process::getTime() + max(60, count($messages) / 2) + (float)$args['observe-seconds'];
    $drain = static function () use ($process, &$positions, &$pending, &$events, &$diagnostics): void {
        $process->drain();
        foreach (array_keys($positions) as $name) {
            $pending[$name] .= substr($process->output[$name], $positions[$name]);
            $positions[$name] = strlen($process->output[$name]);
            while (($end = strpos($pending[$name], "\n")) !== false) {
                $line = substr($pending[$name], 0, $end);
                $pending[$name] = substr($pending[$name], $end + 1);
                if ($name === 'stdout') { $events[] = json_decode($line, true, 512, JSON_THROW_ON_ERROR); }
                else {
                    try { $diagnostics[] = json_decode($line, true, 512, JSON_THROW_ON_ERROR); }
                    catch (\JsonException) { $diagnostics[] = ['diagnostic' => 'stderr', 'message' => $line]; }
                }
            }
        }
    };
    $wait = static function (callable $predicate) use ($process, $drain, $deadline, &$events): void {
        while (!$predicate()) {
            requireCondition(Process::getTime() < $deadline, 'Retained replay exceeded bounded deadline');
            $drain();
            requireCondition(array_filter($events, static fn(array $event): bool =>
                in_array($event['type'] ?? null, ['error', 'frame_rejected'], true)) === [],
                'Renderer rejected retained replay');
            requireCondition($process->isRunning() || $predicate(), 'Renderer exited before acknowledgement');
            usleep(5000);
        }
    };
    $send = static function (array $message) use ($process, &$sent, $deadline): void {
        $line = encodeJson($message) . "\n";
        $sent[] = $line;
        $process->send($line, $deadline);
    };
    try {
        $send($messages[0]);
        $wait(static function () use (&$events): bool {
            return in_array('ready', array_column($events, 'type'), true);
        });
        $presented = 0;
        foreach (array_slice($messages, 1) as $message) {
            $send($message);
            $generation = $message['generation'];
            $wait(static function () use (&$events, $generation): bool {
                return array_filter($events, static fn(array $event): bool =>
                    ($event['type'] ?? null) === 'frame_ack' && ($event['generation'] ?? null) === $generation) !== [];
            });
            if (($message['present'] ?? true) === true) {
                $presented++;
                if ($presented === (int)$args['hold-frame'] && (float)$args['observe-seconds'] > 0) {
                    $frame = $message['frame'];
                    $wait(static function () use (&$diagnostics, $frame): bool {
                        return array_filter($diagnostics, static fn(array $record): bool =>
                            ($record['diagnostic'] ?? null) === 'frame'
                            && ($record['frame'] ?? null) === $frame
                            && ($record['stage'] ?? null) === 'paint_end') !== [];
                    });
                    fwrite(STDOUT, "Retained frame {$presented} painted; silent visual observation window is open.\n");
                    $until = Process::getTime() + (float)$args['observe-seconds'];
                    while (Process::getTime() < $until && $process->isRunning()) { $drain(); usleep(10000); }
                }
            }
        }
        $send(['protocol' => 2, 'type' => 'shutdown']);
        $process->closeInput();
        $wait(static fn(): bool => !$process->isRunning());
        $drain();
        requireCondition($process->exitCode === 0, 'Renderer exited unsuccessfully');
        requireCondition($pending === ['stdout' => '', 'stderr' => ''], 'Incomplete renderer output');
        $receipt = ['binary_sha256' => hash_file('sha256', $binary),
            'replay_sha256' => hash_file('sha256', $args['replay']),
            'packets' => count($messages) - 1, 'presented' => $presented,
            'exit_code' => $process->exitCode, 'silent_fixture_only' => true,
            'measurement' => 'Every generation acknowledged; held frame has CPU paint callback, not GPU completion or FPS'];
        writeFile($args['evidence-dir'] . '/receipt.json', encodeJson($receipt, true) . "\n");
        fwrite(STDOUT, encodeJson($receipt, true) . "\n");
    } finally {
        $process->stop();
        foreach ($process->output as $name => $data) { writeFile($args['evidence-dir'] . '/' . $name . '.ndjson', $data); }
        writeFile($args['evidence-dir'] . '/input.ndjson', implode('', $sent));
    }
});
