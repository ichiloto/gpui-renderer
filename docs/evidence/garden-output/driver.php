<?php

use Ichiloto\Engine\Core\Game;
use Ichiloto\Engine\Diagnostics\LatencyTrace;
use Ichiloto\Engine\IO\Console\Console;
use Ichiloto\Engine\Scenes\Game\GameLoader;
use Ichiloto\Engine\Scenes\Game\GameScene;

require getcwd() . '/vendor/autoload.php';
mt_srand(12345);

final class GardenOutputProbe extends Game
{
  private int $step = 0;
  private float $began;

  protected function startInputSession(): void
  {
    parent::startInputSession();
    // Comparison control only: production GPUI remains buffer-only.
    if (getenv('GARDEN_MIRROR') === '1') {
      Console::setTerminalOutputEnabled(true);
    }
  }

  protected function start(): void
  {
    parent::start();
    $config = GameLoader::getInstance($this)->loadSavedGame(__DIR__ . '/saves/auto-02.iedata');
    if ($config->mapId !== 'overworld/garden-of-roads') {
      throw new RuntimeException('Private checkpoint is not Garden of Roads.');
    }
    $scene = $this->sceneManager->loadScene(GameScene::class)->currentScene;
    assert($scene instanceof GameScene);
    $scene->configure($config);
    $scene->camera->detach();
    $this->began = microtime(true);
    LatencyTrace::record('probe.start', ['mirror' => Console::isTerminalOutputEnabled(),
      'map' => $config->mapId]);
  }

  protected function update(): void
  {
    if ($this->step >= 48 || microtime(true) - $this->began > 30) {
      file_put_contents('probe-result.json', json_encode(['steps' => $this->step,
        'elapsed' => microtime(true) - $this->began, 'mirror' => Console::isTerminalOutputEnabled()]));
      $this->quit();
      return;
    }
    $phase = $this->step < 12 ? 'idle' : 'pan';
    LatencyTrace::record('probe.phase', ['phase' => $phase, 'step' => $this->step]);
    $start = LatencyTrace::now();
    parent::update();
    LatencyTrace::end('probe.update', $start, ['phase' => $phase]);
    if ($phase === 'pan') {
      $scene = $this->sceneManager->currentScene;
      assert($scene instanceof GameScene);
      $scene->camera->moveTo($this->step - 12, $this->step - 12);
      $start = LatencyTrace::now();
      $scene->fieldState->renderTheField();
      LatencyTrace::end('probe.field.recompose', $start, ['phase' => $phase]);
    }
    $this->step++;
  }
}

(new GardenOutputProbe('Ichiloto - Garden output check (muted)',
  options: ['width' => 135, 'height' => 36, 'fps' => 30]))->run();
