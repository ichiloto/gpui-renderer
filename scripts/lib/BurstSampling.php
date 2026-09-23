<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once __DIR__ . '/Process.php';

/** Historical Darwin sampling adapter. Analysis and fixture tools remain platform-neutral. */
function sampleBurstWindow(string $defaultBase): void
{
    $args = parseOptions(['base' => $defaultBase], [],
        'Darwin-only existing-process sampler; never launches a game. Usage: php sample-window.php NAME PID [SECONDS] [--base EVIDENCE_DIR]. Requires PHP FFI for CLOCK_UPTIME_RAW markers.');
    requireCondition(count($args['_']) >= 2 && count($args['_']) <= 3, 'Expected NAME PID [SECONDS]');
    [$name, $pid] = $args['_']; $duration = $args['_'][2] ?? '3';
    requireCondition(preg_match('/^[a-zA-Z0-9._-]+$/', $name) === 1 && ctype_digit($pid) && (int)$pid > 0, 'Invalid sample name or PID');
    requireCondition(is_numeric($duration) && is_finite((float)$duration) && (float)$duration > 0, 'Duration must be positive');
    requireCondition(PHP_OS_FAMILY === 'Darwin', 'This retained sample adapter requires macOS /usr/bin/sample; it is not a portable profiler');
    requireCondition(class_exists(\FFI::class), 'PHP FFI is required to preserve the recorded CLOCK_UPTIME_RAW timebase');
    $clock = \FFI::cdef('unsigned long long clock_gettime_nsec_np(int clock_id);');
    $base = $args['base']; makeDirectory($base);
    $mark = static function ($stage) use ($base, $name, $pid, $clock): void {
        $record = ['stage' => $stage, 'host_ns' => $clock->clock_gettime_nsec_np(8),
            'wall_ns' => (int)round(microtime(true) * 1e9), 'pid' => (int)$pid];
        requireCondition(file_put_contents($base . '/' . $name . '-sample-markers.ndjson', encodeJson($record) . "\n", FILE_APPEND) !== false, 'Cannot write markers');
    };
    $mark('armed'); $deadline = Process::getTime() + 55;
    while (!is_file($base . '/' . $name . '-request')) {
        requireCondition(Process::getTime() < $deadline, 'No burst request within 55 seconds'); usleep(5000);
    }
    $mark('sample_command_begin');
    $command = ['/usr/bin/sample', $pid, $duration, '1', '-file', $base . '/' . $name . '-sample.txt'];
    $process = new Process($command, '', mergeError: true); $ready = false; $position = 0; $pending = '';
    $deadline = Process::getTime() + (float)$duration + 15;
    try {
        do {
            $running = $process->isRunning();
            $process->drain(); $pending .= substr($process->output['stdout'], $position); $position = strlen($process->output['stdout']);
            while (($end = strpos($pending, "\n")) !== false) {
                $line = substr($pending, 0, $end); $pending = substr($pending, $end + 1);
                if (str_contains($line, 'Sampling completed')) { $mark('sample_collection_completed_banner'); }
                if (str_contains($line, 'Sampling process') && !$ready) { $mark('sample_collecting_banner'); writeFile($base . '/' . $name . '-ready', "ready\n"); $ready = true; }
            }
            requireCondition(Process::getTime() < $deadline, 'Sample process exceeded bounded deadline');
            usleep(5000);
        } while ($running);
        $result = $process->wait($deadline);
        $mark('sample_command_end');
        echo encodeJson(['command' => $command, 'returncode' => $result, 'ready' => $ready]), "\n";
    } finally {
        $process->stop(); writeFile($base . '/' . $name . '-sample-console.txt', $process->output['stdout']);
    }
}
