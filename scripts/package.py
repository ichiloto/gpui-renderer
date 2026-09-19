#!/usr/bin/env python3
"""Build and package the GPUI renderer as an installable renderer package.

A renderer package is the artifact `ichiloto renderer:install` consumes: a
.tar.gz (and staged directory) whose root holds renderer-package.json and the
payload tree exactly as it installs under the Engine's
resources/renderers/installed/ boundary, rooted at <renderer>/<platform>/.
Every payload file is listed with its SHA-256 so the installer verifies the
package before staging anything. This script only produces packages; the
Console command owns installation, so the boundary layout lives in one place.

The package format is renderer-agnostic; this script fills it in for the GPUI
renderer. On macOS the payload is the .app bundle (Info.plist preserved for
native application identity); on Linux and Windows it is the bare executable.
Cross-compiled builds pass --platform with --binary pointing at the built
executable for that target.
"""
import argparse
import hashlib
import json
import platform as host_platform
import re
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

RENDERER_ID = 'gpui'
DISPLAY_NAME = 'GPUI'
ROOT = Path(__file__).resolve().parent.parent


def detect_platform() -> str:
    """Mirror the Engine resolver's host platform identity."""
    os_family = {'Darwin': 'darwin', 'Linux': 'linux', 'Windows': 'windows'}.get(
        host_platform.system(), host_platform.system().lower())
    machine = host_platform.machine().lower()
    architecture = {'arm64': 'arm64', 'aarch64': 'arm64', 'x86_64': 'x64', 'amd64': 'x64'}.get(machine, machine)
    return f'{os_family}-{architecture}'


def cargo_version() -> str:
    text = (ROOT / 'Cargo.toml').read_text(encoding='utf-8')
    match = re.search(r'^\s*version\s*=\s*"([^"]+)"', text, re.MULTILINE)
    if not match:
        sys.exit('Cargo.toml declares no package version.')
    return match.group(1)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b''):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--platform', default=detect_platform(),
                        help='Target platform id, e.g. darwin-arm64, linux-x64, windows-x64 (default: this host).')
    parser.add_argument('--binary', type=Path, default=None,
                        help='The built release executable (default: target/release/gpui-renderer[.exe]).')
    parser.add_argument('--out', type=Path, default=ROOT / 'dist',
                        help='Output directory for the staged package and archive (default: dist/).')
    parser.add_argument('--skip-build', action='store_true',
                        help='Package an existing release build without running cargo.')
    args = parser.parse_args()

    if not re.fullmatch(r'[a-z]+-[a-z0-9_]+', args.platform):
        parser.error('--platform must look like darwin-arm64, linux-x64 or windows-x64.')

    if not args.skip_build:
        print('Building the optimized release renderer (cargo build --release --locked)...')
        subprocess.run(['cargo', 'build', '--release', '--locked'], cwd=ROOT, check=True)

    executable_name = 'gpui-renderer.exe' if args.platform.startswith('windows-') else 'gpui-renderer'
    binary = (args.binary or ROOT / 'target/release' / executable_name).resolve()
    if not binary.is_file():
        sys.exit(f'No release executable at {binary}. Build first, or pass --binary.')
    if args.platform != detect_platform() and args.binary is None:
        sys.exit(f'--platform {args.platform} differs from this host ({detect_platform()}); '
                 'pass --binary with the cross-compiled executable.')

    version = cargo_version()
    package_name = f'{RENDERER_ID}-{args.platform}-{version}'
    stage = (args.out / package_name).resolve()
    if stage.exists():
        shutil.rmtree(stage)
    payload_root = stage / RENDERER_ID / args.platform

    if args.platform.startswith('darwin-'):
        contents = payload_root / 'Ichiloto Renderer.app' / 'Contents'
        (contents / 'MacOS').mkdir(parents=True)
        shutil.copy2(ROOT / 'resources/macos/Info.plist', contents / 'Info.plist')
        installed_executable = contents / 'MacOS' / 'gpui-renderer'
    else:
        payload_root.mkdir(parents=True)
        installed_executable = payload_root / executable_name
    shutil.copy2(binary, installed_executable)
    installed_executable.chmod(0o755)

    files = {
        str(path.relative_to(stage)): sha256(path)
        for path in sorted(stage.rglob('*')) if path.is_file()
    }
    descriptor = {
        'version': 1,
        'renderer': RENDERER_ID,
        'displayName': DISPLAY_NAME,
        'platform': args.platform,
        'packageVersion': version,
        'executable': str(installed_executable.relative_to(stage)),
        'files': files,
    }
    (stage / 'renderer-package.json').write_text(
        json.dumps(descriptor, indent=2, sort_keys=False) + '\n', encoding='utf-8')

    archive = args.out / f'{package_name}.tar.gz'
    archive.unlink(missing_ok=True)
    with tarfile.open(archive, 'w:gz') as tar:
        for path in sorted(stage.rglob('*')):
            tar.add(path, arcname=str(path.relative_to(stage)), recursive=False)

    print(f'Staged package directory: {stage}')
    print(f'Package archive:          {archive}')
    print(f'Executable SHA-256:       {files[descriptor["executable"]]}')
    print('Install with: ichiloto renderer:install ' + str(archive)
          + '  (add --engine <engine checkout> for development staging)')


if __name__ == '__main__':
    main()
