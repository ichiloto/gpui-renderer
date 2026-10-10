#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once dirname(__DIR__, 4) . '/scripts/lib/BurstSampling.php';
runCli(static fn() => sampleBurstWindow('/tmp/ichiloto-renderer-burst-release-20260912/evidence'));
