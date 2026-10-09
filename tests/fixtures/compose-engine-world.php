<?php

declare(strict_types=1);

use Ichiloto\Engine\Field\MapGraphics;
use Ichiloto\Engine\Field\MapLayer;
use Ichiloto\Engine\Field\MapLayerSet;
use Ichiloto\Engine\Field\MapTileLayer;
use Ichiloto\Engine\Rendering\Presentation\PresentationWorld;
use Ichiloto\Engine\Rendering\Tilesets\Tileset;

// Reproduce engine-world.json using a live Engine autoloader and an existing
// temporary asset directory. No game content or artwork is used.
if (count($argv) !== 3 || !is_file($argv[1]) || !is_dir($argv[2])) {
    throw new InvalidArgumentException('Usage: php compose-engine-world.php ENGINE_AUTOLOAD TEMP_ASSET_DIRECTORY');
}
require $argv[1];
$root = $argv[2];
if (file_exists($root . '/floor.png')) {
    throw new InvalidArgumentException('The temporary fixture path must not already contain floor.png.');
}
$sheet = imagecreatetruecolor(32, 32);
imagefill($sheet, 0, 0, imagecolorallocate($sheet, 10, 20, 30));
imagepng($sheet, $root . '/floor.png');
try {
    $layers = new MapLayerSet([
        new MapLayer('terrain', 1, false, 'synthetic-terrain', ' .  '),
        new MapLayer('buildings', 2, false, 'synthetic-buildings', '  # '),
        new MapLayer('detail', 3, true, 'synthetic-decoration', 'xxxx'),
    ]);
    $graphics = new MapGraphics(
        Tileset::fromArray('synthetic', ['name' => 'Synthetic', 'sheets' => ['B' => 'floor.png']]),
        [new MapTileLayer('floor', 0, 'synthetic-floor', '1 1 1 0')],
        owners: ['floor' => 'buildings'],
    );
    echo json_encode([
        'operations' => PresentationWorld::getFromLayers($layers, 'map', $graphics, $root)->getOperations(true),
        'shownGlyphs' => $graphics->getShownGlyphCells($layers, $root),
    ], JSON_PRETTY_PRINT | JSON_THROW_ON_ERROR), "\n";
} finally {
    unlink($root . '/floor.png');
}
