#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once __DIR__ . '/lib/Process.php';

runCli(function (): void {
    $args = parseOptions(['binary' => dirname(__DIR__) . '/target/debug/gpui-renderer', 'protocol' => '2', 'pid-file' => null, 'geometry' => 'original'], [],
        'One silent fixture. Usage: php scripts/inspect-fixture.php [--binary PATH] [--protocol 1|2] [--pid-file PATH] [--geometry original|last-legend|oversized|oversized-tall]. Stdin: first, second, invalid, clear, shutdown.');
    requireCondition($args['_'] === [] && in_array($args['protocol'], ['1', '2'], true), 'Expected --protocol 1 or 2');
    $protocol = (int)$args['protocol']; $messages = getFixtureMessages($protocol);
    $geometries = ['original' => null, 'last-legend' => [10, 20], 'oversized' => [20, 40], 'oversized-tall' => [20, 80]];
    requireCondition(array_key_exists($args['geometry'], $geometries), 'Unknown geometry');
    if ($args['geometry'] !== 'original') {
        $columns = 135; $rows = 36; [$cw, $ch] = $geometries[$args['geometry']];
        $messages[0]['grid'] = ['columns' => $columns, 'rows' => $rows, 'cellWidth' => $cw, 'cellHeight' => $ch];
        $messages[0]['title'] = 'Ichiloto - ' . $args['geometry'] . ' viewport (silent)';
        $top = substr('+ ' . "{$columns}x{$rows} / {$cw}x{$ch} / TOP / LEFT " . str_repeat('-', $columns), 0, $columns - 1) . '+';
        $bottom = substr('+ BOTTOM / ALL ROWS AND COLUMNS VISIBLE ' . str_repeat('-', $columns), 0, $columns - 1) . '+';
        for ($i = 1; $i < count($messages); $i++) {
            if ($protocol === 1) {
                $messages[$i]['text'] = array_pad($messages[$i]['text'], $rows, '');
                $messages[$i]['text'][0] = $top; $messages[$i]['text'][$rows - 1] = $bottom;
                for ($row = 1; $row < $rows - 1; $row++) { $messages[$i]['text'][$row] = '|' . str_pad(substr($messages[$i]['text'][$row], 1), $columns - 2) . '|'; }
            } else {
                $runs = [['row' => 0, 'column' => 0, 'text' => $top], ['row' => $rows - 1, 'column' => 0, 'text' => $bottom]];
                for ($row = 1; $row < $rows - 1; $row++) { foreach ([0, $columns - 1] as $column) { $runs[] = ['row' => $row, 'column' => $column, 'text' => '|']; } }
                $runs = array_map(static fn($run) => $run + ['foreground' => ['kind' => 'ansi16', 'index' => 14], 'background' => null], $runs);
                array_unshift($messages[$i]['textLayers'], ['id' => 'viewport-extents', 'layer' => -1, 'runs' => $runs]);
            }
        }
    }
    $process = new Process([$args['binary']]);
    $positions = ['stdout' => 0, 'stderr' => 0];
    $forward = static function () use ($process, &$positions): void {
        $process->drain();
        foreach (['stdout' => STDOUT, 'stderr' => STDERR] as $name => $stream) {
            fwrite($stream, substr($process->output[$name], $positions[$name]));
            $positions[$name] = strlen($process->output[$name]);
        }
    };
    if ($args['pid-file'] !== null) { writeFile($args['pid-file'], $process->pid . "\n"); }
    $send = static fn($message) => $process->send(encodeJson($message) . "\n", Process::getTime() + 5);
    stream_set_blocking(STDIN, false);
    $pending = '';
    try {
        $send($messages[0]); $send($messages[1]);
        $shutdown = false;
        while ($process->isRunning() && !$shutdown) {
            $forward();
            $pending .= stream_get_contents(STDIN);
            if (feof(STDIN) && !str_contains($pending, "\n")) { $pending .= "\n"; }
            while (($end = strpos($pending, "\n")) !== false) {
                $command = trim(substr($pending, 0, $end)); $pending = substr($pending, $end + 1);
                if ($command === 'shutdown' || $command === '') { $send(['protocol' => $protocol, 'type' => 'shutdown']); $shutdown = true; break; }
                if ($command === 'first') { $send($messages[1]); }
                elseif ($command === 'second' && $protocol === 2) { $send($messages[2]); }
                elseif ($command === 'invalid') { $invalid = $messages[1]; $invalid['sprites'][0]['asset'] = 'missing.png'; $send($invalid); }
                elseif ($command === 'clear') { $send(['protocol' => $protocol, 'type' => 'frame', 'frame' => 3, 'sprites' => [], $protocol === 1 ? 'text' : 'textLayers' => []]); }
            }
            usleep(5000);
        }
        $status = $process->wait(Process::getTime() + 5); $forward();
    } finally { $process->stop(); }
    exit($status);
});
