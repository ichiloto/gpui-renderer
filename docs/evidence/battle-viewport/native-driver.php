<?php

use Ichiloto\Engine\Core\Game;
use Ichiloto\Engine\Entities\Troop;
use Ichiloto\Engine\IO\Console\Console;
use Ichiloto\Engine\Scenes\Game\GameLoader;
use Ichiloto\Engine\Scenes\Game\GameScene;

require __DIR__ . '/vendor/autoload.php';
mt_srand(12345);

final class GeometryProbe extends Game
{
    private float $deadline;
    private string $phase = '';
    private GameScene $field;

    protected function start(): void
    {
        parent::start();
        $config = GameLoader::getInstance($this)->loadSavedGame('/tmp/ichiloto-garden-output/saves/auto-02.iedata');
        $this->field = $this->sceneManager->loadScene(GameScene::class)->currentScene;
        $this->field->configure($config);
        $this->deadline = microtime(true) + 180;
    }

    protected function update(): void
    {
        $phase = trim(file_get_contents(__DIR__ . '/phase'));
        if ($phase === 'quit' || microtime(true) > $this->deadline) {
            $this->quit();
            return;
        }
        if ($phase === $this->phase) {
            return;
        }
        $this->phase = $phase;
        if ($phase === 'battle') {
            $troops = require __DIR__ . '/assets/Data/troops.php';
            $this->sceneManager->loadBattleScene($this->field->party, Troop::fromArray($troops[1]));
            $battle = $this->sceneManager->currentScene;
            $battle->setState($battle->runState);
        } else {
            $this->sceneManager->loadScene(GameScene::class);
            $this->field->setState($phase === 'menu' ? $this->field->mainMenuState : $this->field->fieldState);
            if ($phase === 'pan') {
                $this->field->camera->detach();
                $this->field->camera->moveTo(32, 32);
                $this->field->fieldState->renderTheField();
            }
        }
        $camera = $this->sceneManager->currentScene->camera;
        file_put_contents(__DIR__ . '/geometry.ndjson', json_encode([
            'phase' => $phase,
            'grid' => [Console::getWidth(), Console::getHeight()],
            'camera' => [$camera->screen->getWidth(), $camera->screen->getHeight()],
            'terminalOutput' => Console::isTerminalOutputEnabled(),
        ], JSON_THROW_ON_ERROR) . "\n", FILE_APPEND);
    }
}

// Intentionally no width/height: test ordinary CLI renderer selection and defaults.
(new GeometryProbe('Ichiloto - battle geometry check (muted)', options: ['fps' => 30]))->run();
