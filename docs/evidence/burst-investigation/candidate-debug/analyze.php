#!/usr/bin/env php
<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once dirname(__DIR__, 4) . '/scripts/lib/BurstAnalysis.php';
runCli(static fn() => analyzeBurst('/tmp/ichiloto-renderer-burst-candidate-20260912', 56896));
