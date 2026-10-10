#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once __DIR__ . '/lib/Support.php';

runCli(function (): void {
    $args = parseOptions(['protocol' => '1', 'frame' => 'all'], [],
        'Emit deterministic renderer NDJSON. Usage: php scripts/fixture.php [--protocol 1|2] [--frame first|second|all]');
    requireCondition($args['_'] === [] && in_array($args['protocol'], ['1', '2'], true), 'Expected --protocol 1 or 2');
    foreach (getFixtureMessages((int)$args['protocol'], $args['frame']) as $message) {
        echo encodeJson($message), "\n";
        flush();
    }
});
