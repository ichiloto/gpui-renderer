#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once dirname(__DIR__, 2) . '/scripts/lib/Process.php';

runCli(function (): void {
    $root = dirname(__DIR__, 2);
    $tmp = sys_get_temp_dir() . '/ichiloto-php-tools-' . bin2hex(random_bytes(5)); makeDirectory($tmp);
    $checks = 0;
    $check = static function (bool $valid, string $name) use (&$checks): void { requireCondition($valid, $name); $checks++; echo "PASS {$name}\n"; };
    $run = static function (array $command, string $input = '', ?array $env = null, int $expected = 0): array {
        $process = new Process($command, $input, $env);
        try { requireCondition($process->wait(Process::getTime() + 20) === $expected, implode(' ', $command) . ': ' . $process->output['stderr']); return $process->output; }
        finally { $process->stop(); }
    };
    try {
        foreach ([1, 2] as $protocol) {
            foreach (['first', 'second', 'all'] as $frame) {
                $output = $run([PHP_BINARY, $root . '/scripts/fixture.php', '--protocol', (string)$protocol, '--frame', $frame]);
                $actual = array_map(static fn($line) => json_decode($line, true, 512, JSON_THROW_ON_ERROR), explode("\n", trim($output['stdout'])));
                $source = readRecords($root . '/fixtures/' . ($protocol === 1 ? 'home-frame.ndjson' : 'presentation-v2.ndjson'));
                $source[0]['assetRoot'] = $root . '/fixtures';
                $expected = array_values(array_filter($source, static fn($m) => $m['type'] === 'hello' || $frame === 'all' || $m['frame'] === ($frame === 'first' ? 1 : 2)));
                $check($actual === $expected, "fixture v{$protocol} {$frame}");
            }
        }
        $run([PHP_BINARY, $root . '/scripts/fixture.php', '--protocol', '3'], expected: 1);
        $run([PHP_BINARY, $root . '/scripts/fixture.php', '--frame', 'bogus'], expected: 1);
        $trace = $root . '/docs/evidence/viewport-latency/last-legend.stderr.ndjson';
        $report = json_decode($run([PHP_BINARY, $root . '/scripts/analyze-trace.php', $trace, '--events', $root . '/docs/evidence/viewport-latency/last-legend.stdout.ndjson', '--ids', '3:12'])['stdout'], true, 512, JSON_THROW_ON_ERROR);
        $expected = readJson($root . '/docs/evidence/viewport-latency/rapid-right.summary.json');
        unset($report['ordinary_stderr_messages']);
        $check($report == $expected, 'retained callback-to-flush report');
        writeFile($tmp . '/invalid.ndjson', "{\"diagnostic\":broken\n");
        $run([PHP_BINARY, $root . '/scripts/analyze-trace.php', $tmp . '/invalid.ndjson'], expected: 1);
        $check(true, 'malformed diagnostics rejected');
        foreach (['mirror', 'buffer-only', 'normalized-unconfirmed-draw', 'normalized-foreground'] as $name) {
            $base = $root . '/docs/evidence/garden-output/' . $name; $expected = readJson($base . '/summary.json');
            $run([PHP_BINARY, $root . '/docs/evidence/garden-output/analyze.php', $base . '/engine.ndjson', '--pid', (string)$expected['engine_pid'], '--out', $tmp . '/' . $name]);
            $actual = readJson($tmp . '/' . $name . '/summary.json');
            unset($expected['source'], $actual['source']);
            $check($actual == $expected, 'retained garden ' . $name);
        }
        foreach (['baseline', 'candidate-debug', 'candidate-release'] as $name) {
            $base = $root . '/docs/evidence/burst-investigation/' . $name;
            $expected = readJson($base . '/summary.json');
            $session = $tmp . '/burst-' . $name; makeDirectory($session . '/runtime/logs');
            // Reassemble the retained selected Engine records and native stream; no live game is needed.
            $records = readRecords($base . '/engine-selected.ndjson');
            $records[] = ['pid' => $expected['pid'], 'stage' => 'renderer.stderr', 'text' => readFile($base . '/renderer.ndjson')];
            writeFile($session . '/runtime/logs/latency.ndjson', implode('', array_map(static fn($r) => encodeJson($r) . "\n", $records)));
            $run([PHP_BINARY, $base . '/analyze.php', '--base', $session]);
            $check(readJson($session . '/evidence/live-summary.json') == $expected, 'retained burst ' . $name);
            if ($name !== 'baseline') {
                $run([PHP_BINARY, $base . '/correlate.php', '--base', $session . '/evidence']);
                $check(readJson($session . '/evidence/span-correlation.json') == readJson($base . '/span-correlation.json'), 'retained burst correlation ' . $name);
            }
        }
        $echo = $run([PHP_BINARY, '-r', 'fwrite(STDERR,str_repeat("e",200000)); echo stream_get_contents(STDIN);'], str_repeat('x', 200000));
        $check($echo['stdout'] === str_repeat('x', 200000) && $echo['stderr'] === str_repeat('e', 200000), 'large bidirectional capture without pipe deadlock');
        $child = new Process([PHP_BINARY, '-r', 'usleep(5000000);']);
        $timedOut = false;
        try { $child->wait(Process::getTime() + 0.05); } catch (\RuntimeException) { $timedOut = true; } finally { $child->stop(); }
        $check($timedOut, 'bounded timeout and owned-child cleanup');
        if (PHP_OS_FAMILY !== 'Windows') {
            $binary = __DIR__ . '/fake-renderer.php';
            foreach ([[], ['--canvas'], ['--glyphs']] as $mode) {
                $out = $tmp . '/native-' . ($mode[0] ?? 'tiles');
                $run([PHP_BINARY, $root . '/scripts/native-tile-smoke.php', '--binary', $binary, '--evidence-dir', $out, ...$mode]);
                $receipt = readJson($out . '/receipt.json');
                $check($receipt['exit_code'] === 0 && $receipt['frames_painted'] === ($mode === ['--canvas'] ? 5 : 4), 'mock tile driver ' . ($mode[0] ?? 'tiles'));
            }
            foreach ([false, true] as $trace) {
                $output = $run([PHP_BINARY, $root . '/scripts/native-smoke.php', '--binary', $binary, ...($trace ? ['--trace'] : [])], env: array_merge(getenv(), ['FAKE_SMOKE' => '1']));
                $check(substr_count($output['stdout'], 'PASS ') === 13, 'mock IPC smoke ' . ($trace ? 'traced' : 'untraced'));
            }
            foreach ([1, 2] as $protocol) {
                foreach (['original', 'last-legend', 'oversized', 'oversized-tall'] as $geometry) {
                    $capture = $tmp . '/inspect-' . $protocol . '-' . $geometry . '.ndjson';
                    $output = $run([PHP_BINARY, $root . '/scripts/inspect-fixture.php', '--binary', $binary, '--protocol', (string)$protocol, '--geometry', $geometry],
                        "first\nsecond\ninvalid\nclear\nshutdown\n", array_merge(getenv(), ['FAKE_CAPTURE' => $capture, 'ICHILOTO_GPUI_TRACE' => '0']));
                    $sent = readRecords($capture);
                    $check(count($sent) === ($protocol === 1 ? 6 : 7) && end($sent)['type'] === 'shutdown', "mock interactive v{$protocol} {$geometry}");
                    $check(str_contains($output['stdout'], 'ready') && str_contains($output['stdout'], 'error'), 'interactive output forwarded');
                }
            }
        } else { echo "SKIP executable PHP test-double launch on Windows; direct PHP subprocess tests still run.\n"; }
        echo "{$checks} checks passed; no native window or game launched.\n";
    } finally {
        $iterator = new \RecursiveIteratorIterator(new \RecursiveDirectoryIterator($tmp, \FilesystemIterator::SKIP_DOTS), \RecursiveIteratorIterator::CHILD_FIRST);
        foreach ($iterator as $file) { $file->isDir() ? rmdir($file->getPathname()) : unlink($file->getPathname()); }
        rmdir($tmp);
    }
});
