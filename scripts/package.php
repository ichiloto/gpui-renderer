#!/usr/bin/env php
<?php

/**
 * Build and package the GPUI renderer as an installable renderer package.
 *
 * A renderer package is the artifact `ichiloto renderer:install` consumes: a
 * .tar.gz (and staged directory) whose root holds renderer-package.json and
 * the payload tree exactly as it installs under the Engine's
 * resources/renderers/installed/ boundary, rooted at <renderer>/<platform>/.
 * Every payload file is listed with its SHA-256 so the installer verifies the
 * package before staging anything. This script only produces packages; the
 * Console command owns installation, so the boundary layout lives in one place.
 *
 * The package format is renderer-agnostic; this script fills it in for the
 * GPUI renderer. On macOS the payload is the .app bundle (Info.plist preserved
 * for native application identity); on Linux and Windows it is the bare
 * executable. Cross-compiled builds pass --platform with --binary pointing at
 * the built executable for that target.
 */

declare(strict_types=1);

use function Ichiloto\Renderer\Tools\{getAbsolutePackagePath, getPackageFingerprint, buildReleaseExecutable, parseOptions};

require_once __DIR__ . '/lib/PackageBuild.php';
require_once __DIR__ . '/lib/Support.php';

const RENDERER_ID = 'gpui';
const DISPLAY_NAME = 'GPUI';

$root = dirname(__DIR__);

function fail(string $message): never
{
    fwrite(STDERR, $message . PHP_EOL);
    exit(1);
}

/** Mirrors the Engine resolver's host platform identity. */
function detectPlatform(): string
{
    $architecture = match (strtolower(php_uname('m'))) {
        'arm64', 'aarch64' => 'arm64',
        'x86_64', 'amd64' => 'x64',
        default => strtolower(php_uname('m')),
    };

    return strtolower(PHP_OS_FAMILY) . '-' . $architecture;
}

function getCargoVersion(string $root): string
{
    $manifest = (string) file_get_contents($root . '/Cargo.toml');

    if (preg_match('/^\s*version\s*=\s*"([^"]+)"/m', $manifest, $match) !== 1) {
        fail('Cargo.toml declares no package version.');
    }

    return $match[1];
}

function removeTree(string $path): void
{
    if (! file_exists($path)) {
        return;
    }

    if (! is_dir($path) || is_link($path)) {
        @unlink($path);

        return;
    }

    $iterator = new RecursiveIteratorIterator(
        new RecursiveDirectoryIterator($path, FilesystemIterator::SKIP_DOTS),
        RecursiveIteratorIterator::CHILD_FIRST,
    );

    foreach ($iterator as $item) {
        $item->isDir() && ! $item->isLink() ? @rmdir($item->getPathname()) : @unlink($item->getPathname());
    }

    @rmdir($path);
}

function ensureDirectory(string $directory): void
{
    if (! is_dir($directory) && ! @mkdir($directory, 0755, true) && ! is_dir($directory)) {
        fail("Directory {$directory} could not be created.");
    }
}

$usage = <<<USAGE
Usage: php scripts/package.php [options]

Options:
  --platform=<id>   Target platform id, e.g. darwin-arm64, linux-x64,
                    windows-x64 (default: this host).
  --binary=<path>   The built release executable
                    (default: Cargo-reported executable; with --skip-build,
                    target/release/gpui-renderer[.exe]).
  --out=<dir>       Output directory for the staged package and archive
                    (default: dist/).
  --describe        Print build-input JSON only; no build or filesystem writes.
  --skip-build      Package an existing release build without running cargo.
  --help            Show this help.
USAGE;

try {
    $options = parseOptions(array_fill_keys(['platform', 'binary', 'out', 'skip-build', 'describe'], null), ['skip-build', 'describe'], $usage);
    if ($options['_'] !== []) { fail('Unexpected positional arguments.'); }
} catch (Throwable $error) { fail($error->getMessage()); }

foreach (['platform', 'binary', 'out'] as $option) {
    if (isset($options[$option]) && (!is_string($options[$option]) || $options[$option] === '')) {
        fail("--{$option} requires one non-empty value.");
    }
}

if (isset($options['help'])) {
    echo $usage, PHP_EOL;
    exit(0);
}

$platform = is_string($options['platform'] ?? null) ? $options['platform'] : detectPlatform();

if (preg_match('/^[a-z]+-[a-z0-9_]+$/', $platform) !== 1) {
    fail('--platform must look like darwin-arm64, linux-x64 or windows-x64.');
}

try {
    $version = getCargoVersion($root);
    if (preg_match('/^[A-Za-z0-9][A-Za-z0-9.+-]*$/', $version) !== 1) {
        fail('Cargo package version is not safe for a package directory name.');
    }
    $packageName = sprintf('%s-%s-%s', RENDERER_ID, $platform, $version);
    $out = getAbsolutePackagePath(is_string($options['out'] ?? null) ? $options['out'] : $root . '/dist');
    $stage = rtrim($out, '/') . '/' . $packageName;
    if (isset($options['describe'])) {
        if (isset($options['binary']) || isset($options['skip-build'])) {
            fail('--describe cannot certify --binary or --skip-build artifacts.');
        }
        echo json_encode([
            'renderer' => RENDERER_ID,
            'platform' => $platform,
            'profile' => 'release',
            'fingerprint' => getPackageFingerprint($root, $platform, getenv()),
            'packageDirectory' => $stage,
        ], JSON_THROW_ON_ERROR | JSON_UNESCAPED_SLASHES), PHP_EOL;
        exit(0);
    }
    if ($platform !== detectPlatform() && ! isset($options['binary'])) {
        fail(sprintf('--platform %s differs from this host (%s); pass --binary with the cross-compiled executable.', $platform, detectPlatform()));
    }
    $builtBinary = null;
    if (! isset($options['skip-build'])) {
        echo 'Building the optimized release renderer (cargo build --release --locked)...', PHP_EOL;
        $builtBinary = buildReleaseExecutable($root);
    }
    $executableName = str_starts_with($platform, 'windows-') ? 'gpui-renderer.exe' : 'gpui-renderer';
    $binary = is_string($options['binary'] ?? null)
        ? realpath($options['binary'])
        : ($builtBinary ?? realpath($root . '/target/release/' . $executableName));
    if ($binary === false || ! is_file($binary)) {
        fail('No release executable was found. Build first, or pass --binary.');
    }
} catch (Throwable $error) {
    fail($error->getMessage());
}
ensureDirectory($out);
removeTree($stage);
$payloadRoot = $stage . '/' . RENDERER_ID . '/' . $platform;

if (str_starts_with($platform, 'darwin-')) {
    $contents = $payloadRoot . '/Ichiloto Renderer.app/Contents';
    ensureDirectory($contents . '/MacOS');

    if (! copy($root . '/resources/macos/Info.plist', $contents . '/Info.plist')) {
        fail('resources/macos/Info.plist could not be staged.');
    }

    $installedExecutable = $contents . '/MacOS/gpui-renderer';
} else {
    ensureDirectory($payloadRoot);
    $installedExecutable = $payloadRoot . '/' . $executableName;
}

if (! copy($binary, $installedExecutable) || ! chmod($installedExecutable, 0755)) {
    fail('The release executable could not be staged.');
}

$files = [];
$iterator = new RecursiveIteratorIterator(
    new RecursiveDirectoryIterator($stage, FilesystemIterator::SKIP_DOTS),
);

foreach ($iterator as $item) {
    if ($item->isFile()) {
        $files[str_replace('\\', '/', substr($item->getPathname(), strlen($stage) + 1))] = hash_file('sha256', $item->getPathname());
    }
}

ksort($files);
$executableRelative = str_replace('\\', '/', substr($installedExecutable, strlen($stage) + 1));
$descriptor = [
    'version' => 1,
    'renderer' => RENDERER_ID,
    'displayName' => DISPLAY_NAME,
    'platform' => $platform,
    'packageVersion' => $version,
    'executable' => $executableRelative,
    'files' => $files,
];
file_put_contents(
    $stage . '/renderer-package.json',
    json_encode($descriptor, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES) . "\n",
);

$tarFile = $out . '/' . $packageName . '.tar';
$archiveFile = $tarFile . '.gz';
@unlink($tarFile);
@unlink($archiveFile);
$archive = new PharData($tarFile);
$archive->buildFromDirectory($stage);
$archive->compress(Phar::GZ);
unset($archive);
Phar::unlinkArchive($tarFile);

echo 'Staged package directory: ', $stage, PHP_EOL;
echo 'Package archive:          ', $archiveFile, PHP_EOL;
echo 'Executable SHA-256:       ', $files[$executableRelative], PHP_EOL;
