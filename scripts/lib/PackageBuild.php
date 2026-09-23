<?php

declare(strict_types=1);

namespace Ichiloto\Renderer\Tools;

use RuntimeException;

/** Normalize even an output directory which does not exist yet, without writing. */
function getAbsolutePackagePath(string $path, ?string $base = null): string
{
    if ($path === '') { throw new RuntimeException('The output directory must not be empty.'); }
    if (PHP_OS_FAMILY === 'Windows') { $path = str_replace('\\', '/', $path); }
    if (!str_starts_with($path, '/') && preg_match('~^[A-Za-z]:/~', $path) !== 1) {
        if (PHP_OS_FAMILY === 'Windows' && preg_match('~^[A-Za-z]:~', $path)) {
            throw new RuntimeException('Use an absolute Windows drive path.');
        }
        $path = str_replace('\\', '/', $base ?? (string)getcwd()) . '/' . $path;
    }
    $prefix = str_starts_with($path, '//') ? '//' : '/';
    if (preg_match('~^([A-Za-z]:)/~', $path, $match)) {
        $prefix = $match[1] . '/'; $path = substr($path, 3);
    }
    $parts = [];
    foreach (explode('/', $path) as $part) {
        if ($part === '' || $part === '.') { continue; }
        if ($part === '..') { array_pop($parts); } else { $parts[] = $part; }
    }
    return $prefix . implode('/', $parts);
}

/** Source and declared build context, not a claim of hermetic compiler provenance. */
function getPackageFingerprint(string $root, string $platform, array $environment): string
{
    $inputs = [];
    $add = static function (string $key, string $path) use (&$inputs): void {
        $hash = hash_file('sha256', $path);
        if ($hash === false) { throw new RuntimeException("Cannot read build input: {$path}"); }
        $inputs[$key] = $hash;
    };
    foreach (['Cargo.toml', 'Cargo.lock', 'scripts/package.php', 'scripts/lib/PackageBuild.php', 'scripts/lib/Support.php'] as $file) {
        $add($file, $root . '/' . $file);
    }
    foreach (['build.rs', 'rust-toolchain', 'rust-toolchain.toml'] as $file) {
        if (is_file($root . '/' . $file)) { $add($file, $root . '/' . $file); }
    }
    foreach (['src', 'resources'] as $directory) {
        $path = $root . '/' . $directory;
        if (is_link($path)) { throw new RuntimeException('Linked build-input directories are unsupported: ' . $path); }
        if (!is_dir($path)) { continue; }
        $iterator = new \RecursiveIteratorIterator(new \RecursiveDirectoryIterator($path, \FilesystemIterator::SKIP_DOTS));
        foreach ($iterator as $file) {
            // Directory links could hide arbitrary inputs or cycles. Fail rather than certify an incomplete tree.
            if ($file->isLink() && !$file->isFile()) { throw new RuntimeException('Linked build-input directories or broken links are unsupported: ' . $file->getPathname()); }
            if ($file->isFile()) { $add(str_replace('\\', '/', substr($file->getPathname(), strlen($root) + 1)), $file->getPathname()); }
        }
    }
    $configDirectories = [];
    for ($directory = $root; ; $directory = dirname($directory)) {
        $configDirectories[] = $directory . '/.cargo';
        if (dirname($directory) === $directory) { break; }
    }
    $home = $environment['HOME'] ?? $environment['USERPROFILE'] ?? null;
    $cargoHome = $environment['CARGO_HOME'] ?? ($home === null ? null : $home . '/.cargo');
    if ($cargoHome !== null) { $configDirectories[] = getAbsolutePackagePath($cargoHome, $root); }
    foreach (array_unique($configDirectories) as $directory) {
        foreach (['config', 'config.toml'] as $name) {
            $path = $directory . '/' . $name;
            if (is_file($path)) { $add('cargo-config:' . $path, $path); }
        }
    }
    ksort($inputs);
    $context = [];
    foreach ($environment as $name => $value) {
        if (preg_match('/^(PATH|PATHEXT|RUSTFLAGS|CARGO_ENCODED_RUSTFLAGS|RUSTC|RUSTC_WRAPPER|RUSTC_WORKSPACE_WRAPPER|RUSTUP_TOOLCHAIN|RUSTUP_HOME|CARGO_HOME|CARGO_TARGET_DIR|CARGO_BUILD_.*|CARGO_TARGET_.*|CARGO_PROFILE_.*|CC(?:_.*)?|CXX(?:_.*)?|AR(?:_.*)?|CFLAGS(?:_.*)?|CXXFLAGS(?:_.*)?|LDFLAGS(?:_.*)?|SDKROOT|MACOSX_DEPLOYMENT_TARGET|PKG_CONFIG(?:_.*)?|INCLUDE|LIB|LIBPATH)$/', $name)) {
            $context[$name] = $value;
        }
    }
    ksort($context);
    return hash('sha256', json_encode(['schema' => 1, 'platform' => $platform, 'profile' => 'release', 'inputs' => $inputs, 'environment' => $context], JSON_THROW_ON_ERROR | JSON_INVALID_UTF8_SUBSTITUTE));
}

/** Force a native build even if Cargo configuration selects a foreign target. */
function getNativeBuildCommand(string $root, ?array $compilerCommand = null): array
{
    $compilerCommand ??= [getenv('RUSTC') ?: 'rustc', '-vV'];
    $process = proc_open($compilerCommand, [0 => ['pipe', 'r'], 1 => ['pipe', 'w'], 2 => STDERR], $pipes, $root, null, ['bypass_shell' => true]);
    if (!is_resource($process)) { throw new RuntimeException('Cannot query the native Rust host.'); }
    fclose($pipes[0]);
    $version = stream_get_contents($pipes[1]);
    fclose($pipes[1]);
    $status = proc_close($process);
    if ($status !== 0 || !is_string($version) || preg_match('/^host: ([a-zA-Z0-9_]+(?:-[a-zA-Z0-9_.]+)+)\r?$/m', $version, $match) !== 1) {
        throw new RuntimeException('Rust did not report a valid native host triple.');
    }
    $architecture = match (strtolower(php_uname('m'))) {
        'arm64', 'aarch64' => 'aarch64',
        'amd64', 'x86_64' => 'x86_64',
        default => strtolower(php_uname('m')),
    };
    if (!str_starts_with($match[1], $architecture . '-') || !in_array(strtolower(PHP_OS_FAMILY), explode('-', $match[1]), true)) {
        throw new RuntimeException('Rust host differs from the package host; use a native toolchain or explicitly package a cross-compiled --binary.');
    }
    return ['cargo', 'build', '--release', '--locked', '--target=' . $match[1], '--message-format=json-render-diagnostics'];
}

/** Cargo owns target-directory resolution. Keep diagnostics human-readable. */
function buildReleaseExecutable(string $root, ?array $command = null): string
{
    $command ??= getNativeBuildCommand($root);
    $process = proc_open($command, [0 => ['pipe', 'r'], 1 => ['pipe', 'w'], 2 => STDERR], $pipes, $root, null, ['bypass_shell' => true]);
    if (!is_resource($process)) { throw new RuntimeException('Cannot start Cargo.'); }
    fclose($pipes[0]);
    $binary = null;
    while (($line = fgets($pipes[1])) !== false) {
        $message = json_decode($line, true);
        if (!is_array($message)) { echo $line; continue; }
        if (($message['reason'] ?? null) === 'compiler-message') {
            fwrite(STDERR, $message['message']['rendered'] ?? ($message['message']['message'] ?? '') . PHP_EOL);
        }
        if (($message['reason'] ?? null) === 'compiler-artifact'
            && ($message['target']['name'] ?? null) === 'gpui-renderer'
            && in_array('bin', $message['target']['kind'] ?? [], true)
            && !($message['profile']['test'] ?? false)
            && is_string($message['executable'] ?? null)) {
            $binary = $message['executable'];
        }
    }
    fclose($pipes[1]);
    $status = proc_close($process);
    if ($status !== 0) { throw new RuntimeException('The cargo build failed.'); }
    if ($binary === null || !is_file($binary)) { throw new RuntimeException('Cargo reported no gpui-renderer release executable.'); }
    return $binary;
}
