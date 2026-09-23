<?php

declare(strict_types=1);

namespace Ichiloto\Renderer\Tools;

/** Small CLI helpers shared by the standalone PHP tools (no Composer dependencies). */
function parseOptions(array $defaults, array $flags, string $usage): array
{
    $options = $defaults;
    $options['_'] = [];
    $args = array_slice($_SERVER['argv'], 1);
    for ($i = 0; $i < count($args); $i++) {
        $arg = $args[$i];
        if ($arg === '--help' || $arg === '-h') { echo $usage, PHP_EOL; exit(0); }
        if ($arg === '--') { array_push($options['_'], ...array_slice($args, $i + 1)); break; }
        if (!str_starts_with($arg, '--')) { $options['_'][] = $arg; continue; }
        [$name, $value] = array_pad(explode('=', substr($arg, 2), 2), 2, null);
        requireCondition(array_key_exists($name, $defaults), "Unknown option --{$name}\n{$usage}");
        if (in_array($name, $flags, true)) {
            requireCondition($value === null, "--{$name} takes no value");
            $options[$name] = true;
        } else {
            $value ??= $args[++$i] ?? null;
            requireCondition($value !== null && !str_starts_with($value, '--'), "--{$name} requires a value");
            $options[$name] = $value;
        }
    }
    return $options;
}

function requireCondition(bool $condition, string $message): void
{
    if (!$condition) { throw new \RuntimeException($message); }
}

function readJson(string $path): array
{
    return json_decode(readFile($path), true, 512, JSON_THROW_ON_ERROR);
}

function readFile(string $path): string
{
    $data = file_get_contents($path);
    requireCondition($data !== false, "Cannot read {$path}");
    return $data;
}

function encodeJson(mixed $value, bool $pretty = false): string
{
    return json_encode($value, JSON_THROW_ON_ERROR | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE
        | ($pretty ? JSON_PRETTY_PRINT : 0));
}

function readRecords(string $path): array
{
    return array_map(static fn($line) => json_decode($line, true, 512, JSON_THROW_ON_ERROR),
        array_values(array_filter(explode("\n", readFile($path)), static fn($line) => trim($line) !== '')));
}

function makeDirectory(string $path): void
{
    requireCondition(is_dir($path) || mkdir($path, 0777, true), "Cannot create {$path}");
}

function writeFile(string $path, string $data): void
{
    requireCondition(file_put_contents($path, $data) === strlen($data), "Cannot write {$path}");
}

function getMedian(array $values): float|int
{
    sort($values, SORT_NUMERIC);
    $count = count($values);
    requireCondition($count > 0, 'Median requires values');
    return $count % 2 ? $values[intdiv($count, 2)] : ($values[$count / 2 - 1] + $values[$count / 2]) / 2;
}

function getDistribution(array $values): ?array
{
    return $values === [] ? null : ['median' => getMedian($values), 'maximum' => max($values)];
}

function getMillisecondsDistribution(array $values): ?array
{
    if ($values === []) { return null; }
    sort($values, SORT_NUMERIC);
    return ['n' => count($values), 'median_ms' => getMedian($values),
        'p95_ms' => $values[max(0, (int)ceil(0.95 * count($values)) - 1)], 'max_ms' => max($values)];
}

function getFixtureMessages(int $protocol = 1, string $frame = 'all'): array
{
    requireCondition(in_array($protocol, [1, 2], true), '--protocol must be 1 or 2');
    requireCondition(in_array($frame, ['first', 'second', 'all'], true), '--frame must be first, second or all');
    $root = dirname(__DIR__, 2) . '/fixtures';
    $result = [];
    foreach (readRecords($root . ($protocol === 1 ? '/home-frame.ndjson' : '/presentation-v2.ndjson')) as $message) {
        if ($message['type'] === 'hello') { $message['assetRoot'] = $root; }
        elseif ($frame !== 'all' && $message['frame'] !== ($frame === 'first' ? 1 : 2)) { continue; }
        $result[] = $message;
    }
    return $result;
}

function runCli(callable $run): void
{
    try { $run(); }
    catch (\Throwable $error) { fwrite(STDERR, $error->getMessage() . PHP_EOL); exit(1); }
}
