<?php

declare(strict_types=1);
namespace Ichiloto\Renderer\Tools;
require_once __DIR__ . '/Support.php';

/** Owns one child, bounded pipe writes and cleanup. File-backed capture cannot fill a pipe. */
final class Process
{
    private $process;
    private array $pipes = [];
    private array $files = [];
    private array $readers = [];
    public array $output = ['stdout' => '', 'stderr' => ''];
    public ?int $exitCode = null;
    public readonly int $pid;

    public function __construct(array $command, ?string $input = null, ?array $environment = null, string $stdout = 'capture', bool $mergeError = false)
    {
        $descriptors = [];
        if ($input === null) { $descriptors[0] = ['pipe', 'r']; }
        else {
            $file = tmpfile(); requireCondition($file !== false, 'Cannot create input file');
            $this->files[] = $file;
            requireCondition(fwrite($file, $input) === strlen($input), 'Cannot write input file');
            rewind($file); $descriptors[0] = $file;
        }
        foreach ([1 => 'stdout', 2 => 'stderr'] as $fd => $name) {
            if ($fd === 2 && $mergeError) { $descriptors[2] = ['redirect', 1]; continue; }
            if ($fd === 1 && in_array($stdout, ['closed', 'unread'], true)) { $descriptors[$fd] = ['pipe', 'w']; continue; }
            $file = tmpfile(); requireCondition($file !== false, 'Cannot create capture file');
            $this->files[] = $file;
            $reader = fopen(stream_get_meta_data($file)['uri'], 'rb');
            requireCondition($reader !== false, 'Cannot open capture reader');
            $this->readers[$name] = $reader; $descriptors[$fd] = $file;
        }
        $this->process = proc_open($command, $descriptors, $this->pipes, null, $environment, ['bypass_shell' => true]);
        requireCondition(is_resource($this->process), 'Cannot start child process');
        $this->pid = proc_get_status($this->process)['pid'];
        if (isset($this->pipes[0])) { stream_set_blocking($this->pipes[0], false); }
        if ($stdout === 'closed') { fclose($this->pipes[1]); unset($this->pipes[1]); }
    }

    public function isRunning(): bool
    {
        if ($this->exitCode !== null) { return false; }
        $status = proc_get_status($this->process);
        if (!$status['running']) { $this->exitCode = $status['exitcode']; return false; }
        return true;
    }

    public function drain(): void
    {
        foreach ($this->readers as $name => $reader) {
            $data = stream_get_contents($reader);
            requireCondition($data !== false, 'Cannot read child output');
            $this->output[$name] .= $data;
        }
    }

    public function send(string $data, float $deadline): void
    {
        requireCondition(isset($this->pipes[0]), 'Child stdin is closed');
        $offset = 0;
        while ($offset < strlen($data)) {
            requireCondition(self::getTime() < $deadline, 'Renderer input exceeded deadline');
            requireCondition($this->isRunning(), 'Renderer exited while writing input');
            $written = @fwrite($this->pipes[0], substr($data, $offset, 65536));
            requireCondition($written !== false, 'Cannot write renderer input');
            $offset += $written;
            $this->drain();
            if ($written === 0) { usleep(1000); }
        }
        fflush($this->pipes[0]);
    }

    public function closeInput(): void
    {
        if (isset($this->pipes[0])) { fclose($this->pipes[0]); unset($this->pipes[0]); }
    }

    public function wait(float $deadline): int
    {
        while ($this->isRunning()) {
            requireCondition(self::getTime() < $deadline, 'Child exceeded bounded deadline');
            $this->drain(); usleep(5000);
        }
        $this->drain();
        return $this->exitCode;
    }

    public function stop(): void
    {
        if (!is_resource($this->process)) { return; }
        $this->closeInput();
        if ($this->isRunning()) {
            proc_terminate($this->process);
            $deadline = self::getTime() + 3;
            while ($this->isRunning() && self::getTime() < $deadline) { $this->drain(); usleep(10000); }
            if ($this->isRunning()) { proc_terminate($this->process, 9); }
        }
        $this->drain();
        foreach ($this->pipes as $pipe) { if (is_resource($pipe)) { fclose($pipe); } }
        proc_close($this->process);
        foreach ([...array_values($this->readers), ...$this->files] as $file) { fclose($file); }
        $this->process = null;
    }

    public static function getTime(): float { return hrtime(true) / 1e9; }
    public function __destruct() { $this->stop(); }
}
