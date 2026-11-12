#!/usr/bin/env python3
"""Build only the shipped Roslyn helper with local SDK/packs; no NuGet sources."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent


def dotnet_path():
    candidate = os.environ.get('AI_ENV_DOTNET') or shutil.which('dotnet')
    if not candidate:
        candidate = next((str(p) for p in (Path('/usr/local/share/dotnet/dotnet'), Path('/usr/share/dotnet/dotnet')) if p.is_file()), None)
    if not candidate:
        raise RuntimeError('.NET 8 SDK is required; install it and rerun analyzers/csharp/build.py')
    return str(Path(candidate).resolve())


def publish(staged, target):
    """Swap a complete fresh directory; keep the previous helper on failure."""
    backup = None
    warnings = []
    if target.exists():
        if not target.is_dir() or target.is_symlink():
            raise RuntimeError('Helper output must be a regular directory')
        backup = Path(tempfile.mkdtemp(prefix='.roslyn-previous-', dir=target.parent))
        try:
            os.replace(target, backup)
        except BaseException:
            backup.rmdir()
            raise
    try:
        os.replace(staged, target)
    except BaseException as error:
        if backup is not None:
            try:
                os.replace(backup, target)
            except OSError as restore:
                raise RuntimeError(f'Helper publication failed: {error}; rollback failed: {restore}; previous helper retained at {backup}') from error
        raise
    if backup is not None:
        try:
            shutil.rmtree(backup)
        except OSError as error:
            warnings.append(f'Previous helper cleanup failed; retained {backup}: {error}')
    return warnings


def build(output=None):
    dotnet = dotnet_path()
    target = Path(output).resolve() if output else HERE.parents[1] / 'bin' / 'roslyn'
    sdks = subprocess.run([dotnet, '--list-sdks'], capture_output=True, text=True, check=True, timeout=15).stdout
    versions = re.findall(r'^(8\.\d+\.\d+)\s+\[', sdks, re.MULTILINE)
    if not versions:
        raise RuntimeError('A local .NET 8 SDK and targeting pack are required; no network restore is performed')
    version = max(versions, key=lambda v: tuple(map(int, v.split('.'))))
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.roslyn-output-', dir=target.parent) as fresh, tempfile.TemporaryDirectory(prefix='aienv-roslyn-build-') as temp:
        staged = Path(fresh) / 'helper'
        workspace = Path(temp)
        for name in ('GraphAnalyzer.csproj', 'Program.cs'):
            shutil.copyfile(HERE / name, workspace / name)
        (workspace / 'global.json').write_text(json.dumps({'sdk': {'version': version, 'rollForward': 'disable'}}))
        (workspace / 'NuGet.Config').write_text('<configuration><packageSources><clear /></packageSources></configuration>')
        env = {**os.environ, 'DOTNET_CLI_HOME': str(workspace / 'home'),
               'DOTNET_SKIP_FIRST_TIME_EXPERIENCE': '1', 'DOTNET_CLI_TELEMETRY_OPTOUT': '1',
               'DOTNET_NOLOGO': '1', 'DOTNET_MULTILEVEL_LOOKUP': '0',
               'NUGET_PACKAGES': str(workspace / 'packages')}
        props = ['-p:ImportDirectoryBuildProps=false', '-p:ImportDirectoryBuildTargets=false',
                 '-p:ImportDirectoryPackagesProps=false', '-p:UseSharedCompilation=false',
                 '-p:NuGetAudit=false', '-p:SatelliteResourceLanguages=en']
        commands = [
            [dotnet, 'restore', 'GraphAnalyzer.csproj', '--configfile', 'NuGet.Config', *props],
            [dotnet, 'build', 'GraphAnalyzer.csproj', '--no-restore', '--configuration', 'Release', '--output', str(staged), *props],
        ]
        for argv in commands:
            result = subprocess.run(argv, cwd=workspace, env=env, capture_output=True, text=True, timeout=120)
            if result.returncode:
                raise RuntimeError('Roslyn helper build failed (offline):\n' + result.stdout[-12000:] + result.stderr[-6000:])
        # Compiler diagnostics explicitly use invariant culture. SDK references
        # copy satellite resources even without NuGet; none is needed at runtime.
        for directory in staged.iterdir():
            if directory.is_dir():
                resources = [p for p in directory.rglob('*') if p.is_file()]
                if any(not p.name.endswith('.resources.dll') for p in resources):
                    raise RuntimeError(f'Unexpected helper runtime directory: {directory.name}')
                shutil.rmtree(directory)
        warnings = publish(staged, target)
    return {'built': True, 'sdk': version, 'helper': str(target / 'GraphAnalyzer.dll'), 'dotnet': dotnet, 'cleanup_warnings': warnings}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output')
    args = parser.parse_args()
    try:
        print(json.dumps(build(args.output)))
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
