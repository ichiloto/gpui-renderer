#!/usr/bin/env php
<?php

declare(strict_types=1);

namespace Ichiloto\Renderer\Tools;

require_once dirname(__DIR__, 2) . '/scripts/lib/Process.php';
require_once dirname(__DIR__, 2) . '/scripts/lib/PackageBuild.php';

runCli(function (): void {
    $root = dirname(__DIR__, 2);
    $tmp = sys_get_temp_dir() . '/ichiloto-package-' . bin2hex(random_bytes(5));
    $source = $tmp . '/source';
    makeDirectory($source . '/scripts/lib');
    makeDirectory($source . '/src');
    makeDirectory($source . '/resources/macos');
    copy($root . '/scripts/package.php', $source . '/scripts/package.php');
    copy($root . '/scripts/lib/PackageBuild.php', $source . '/scripts/lib/PackageBuild.php');
    copy($root . '/scripts/lib/Support.php', $source . '/scripts/lib/Support.php');
    writeFile($source . '/Cargo.toml', "[package]\nname = \"gpui-renderer\"\nversion = \"0.1.0\"\n");
    writeFile($source . '/Cargo.lock', 'synthetic lock');
    writeFile($source . '/src/main.rs', 'synthetic source');
    writeFile($source . '/resources/macos/Info.plist', '<plist/>');
    $checks = 0;
    $check = static function (bool $valid, string $label) use (&$checks): void {
        requireCondition($valid, $label); $checks++; echo "PASS {$label}\n";
    };
    $run = static function (array $arguments, int $expected = 0, bool $describe = true) use ($source): array {
        // Description must still work with every process-launch API disabled.
        $command = [PHP_BINARY, ...($describe ? ['-d', 'disable_functions=proc_open,passthru,exec,shell_exec,system,popen'] : []), $source . '/scripts/package.php', ...$arguments];
        $process = new Process($command, '');
        try {
            requireCondition($process->wait(Process::getTime() + 15) === $expected, $process->output['stderr']);
            return $process->output;
        } finally { $process->stop(); }
    };
    try {
        $out = $tmp . '/private output';
        $arguments = ['--describe', '--out=' . $out];
        $output = $run($arguments);
        $description = json_decode($output['stdout'], true, 512, JSON_THROW_ON_ERROR);
        $check(array_keys($description) === ['renderer', 'platform', 'profile', 'fingerprint', 'packageDirectory'] && $description['renderer'] === 'gpui' && $description['profile'] === 'release' && preg_match('/^[a-f0-9]{64}$/', $description['fingerprint']) === 1, 'exact five-field description');
        $check($output['stderr'] === '' && substr_count($output['stdout'], "\n") === 1 && !file_exists($out) && !file_exists($source . '/target') && !file_exists($source . '/dist'), 'JSON only with no build or output writes');
        $check($output === $run($arguments), 'repeatable description');
        $check($description['packageDirectory'] === $out . '/gpui-' . $description['platform'] . '-0.1.0', 'existing package naming inside private output');
        $relative = json_decode($run(['--describe', '--out=./missing/../package-output'])['stdout'], true, 512, JSON_THROW_ON_ERROR);
        $check($relative['packageDirectory'] === getAbsolutePackagePath('./package-output') . '/gpui-' . $description['platform'] . '-0.1.0', 'relative output normalized before directory exists');
        foreach (['linux-x64', 'windows-x64', 'darwin-arm64'] as $platform) {
            $other = json_decode($run([...$arguments, '--platform=' . $platform])['stdout'], true, 512, JSON_THROW_ON_ERROR);
            $check($other['platform'] === $platform && str_ends_with($other['packageDirectory'], '/gpui-' . $platform . '-0.1.0'), 'description platform ' . $platform);
        }
        foreach (['--binary=unused', '--skip-build', '--platform=../bad', '--out='] as $invalid) {
            $failure = $run(['--describe', $invalid], 1);
            $check($failure['stdout'] === '' && $failure['stderr'] !== '', 'reject unsafe description ' . $invalid);
        }
        $env = getenv();
        $fingerprint = static fn() => getPackageFingerprint($source, 'linux-x64', $env);
        $before = $fingerprint();
        foreach (['Cargo.toml', 'Cargo.lock', 'src/main.rs', 'resources/macos/Info.plist', 'scripts/package.php', 'scripts/lib/PackageBuild.php'] as $file) {
            $path = $source . '/' . $file; $saved = readFile($path);
            writeFile($path, $saved . "\nchanged");
            $check($fingerprint() !== $before, 'fingerprint detects edit ' . $file);
            writeFile($path, $saved);
        }
        foreach (['src/extra.rs', 'build.rs', 'rust-toolchain.toml', '.cargo/config.toml'] as $file) {
            $path = $source . '/' . $file; makeDirectory(dirname($path)); writeFile($path, 'new input');
            $changed = $fingerprint(); unlink($path);
            $check($changed !== $before && $fingerprint() === $before, 'fingerprint detects addition and deletion ' . $file);
        }
        foreach (['README.md', 'target/cache', 'dist/package', 'docs/evidence/trace', '.git/HEAD', 'game/art.png'] as $file) {
            $path = $source . '/' . $file; makeDirectory(dirname($path)); writeFile($path, 'irrelevant');
        }
        $check($fingerprint() === $before, 'docs output caches git and game art excluded');
        $check(getPackageFingerprint($source, 'linux-x64', array_merge($env, ['RUSTFLAGS' => 'changed'])) !== $before, 'build environment included');
        $check(getPackageFingerprint($source, 'linux-x64', array_merge($env, ['ICHILOTO_GAME_ART' => 'changed'])) === $before, 'unrelated runtime environment excluded');
        makeDirectory($tmp . '/.cargo'); writeFile($tmp . '/.cargo/config.toml', '[build]');
        $check($fingerprint() !== $before, 'ancestor Cargo configuration included');
        unlink($tmp . '/.cargo/config.toml');
        makeDirectory($tmp . '/cargo-home'); writeFile($tmp . '/cargo-home/config.toml', '[build]');
        $configured = array_merge($env, ['CARGO_HOME' => $tmp . '/cargo-home']);
        $first = getPackageFingerprint($source, 'linux-x64', $configured);
        writeFile($tmp . '/cargo-home/config.toml', '[target]');
        $check(getPackageFingerprint($source, 'linux-x64', $configured) !== $first, 'Cargo home configuration included');
        $binary = $tmp . '/custom target/triple/release/gpui-renderer'; makeDirectory(dirname($binary)); writeFile($binary, 'synthetic executable');
        $artifact = ['reason' => 'compiler-artifact', 'target' => ['name' => 'gpui-renderer', 'kind' => ['bin']], 'profile' => ['test' => false], 'executable' => $binary];
        $fake = $tmp . '/fake-cargo.php';
        $compiler = $tmp . '/fake-rustc.php';
        $architecture = match (strtolower(php_uname('m'))) { 'arm64', 'aarch64' => 'aarch64', 'amd64', 'x86_64' => 'x86_64', default => strtolower(php_uname('m')) };
        $host = $architecture . '-test-' . strtolower(PHP_OS_FAMILY);
        writeFile($compiler, '<?php echo ' . var_export("rustc test\nhost: {$host}\n", true) . ';');
        $command = getNativeBuildCommand($source, [PHP_BINARY, $compiler]);
        $check($command === ['cargo', 'build', '--release', '--locked', '--target=' . $host, '--message-format=json-render-diagnostics'], 'native host target explicitly overrides Cargo target configuration');
        foreach (['<?php echo "host: alien-test-foreign\n";', '<?php echo "rustc without host";', '<?php exit(1);'] as $script) {
            writeFile($compiler, $script); $failed = false;
            try { getNativeBuildCommand($source, [PHP_BINARY, $compiler]); } catch (\RuntimeException) { $failed = true; }
            $check($failed, 'reject foreign missing or failed compiler host discovery');
        }
        $emit = static function (array $records, int $status = 0) use ($fake): void {
            $text = implode('', array_map(static fn($r) => json_encode($r, JSON_THROW_ON_ERROR) . "\n", $records));
            writeFile($fake, '<?php echo ' . var_export($text, true) . '; exit(' . $status . ');');
        };
        $emit([['reason' => 'compiler-artifact', 'target' => ['name' => 'dependency', 'kind' => ['bin']], 'executable' => '/wrong'], $artifact, ['reason' => 'build-finished', 'success' => true]]);
        $check(buildReleaseExecutable($source, [PHP_BINARY, $fake]) === $binary, 'Cargo artifact selects custom target executable');
        foreach ([[[$artifact], 1, 'failed build'], [[], 0, 'missing artifact'], [[array_replace($artifact, ['executable' => '/missing'])], 0, 'missing executable'], [[array_replace($artifact, ['profile' => ['test' => true]])], 0, 'test artifact']] as [$records, $status, $label]) {
            $emit($records, $status); $failed = false;
            try { buildReleaseExecutable($source, [PHP_BINARY, $fake]); } catch (\RuntimeException) { $failed = true; }
            $check($failed, 'reject ' . $label);
        }
        $run(['--skip-build', '--binary=' . $binary, '--out=' . $out], describe: false);
        $package = readJson($description['packageDirectory'] . '/renderer-package.json');
        $check(is_file($description['packageDirectory'] . '.tar.gz') && hash_file('sha256', $description['packageDirectory'] . '/' . $package['executable']) === hash_file('sha256', $binary), 'manual package staged at described path with payload hash');
        $check(json_decode($run($arguments)['stdout'], true)['fingerprint'] === $description['fingerprint'], 'packaging output does not change fingerprint');
        echo "{$checks} package checks passed; no Cargo or native game launched.\n";
    } finally {
        $iterator = new \RecursiveIteratorIterator(new \RecursiveDirectoryIterator($tmp, \FilesystemIterator::SKIP_DOTS), \RecursiveIteratorIterator::CHILD_FIRST);
        foreach ($iterator as $file) { $file->isDir() && !$file->isLink() ? rmdir($file->getPathname()) : unlink($file->getPathname()); }
        rmdir($tmp);
    }
});
