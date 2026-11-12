#!/usr/bin/env python3
"""One JSON request -> one Roslyn graph. Never build/execute the target project."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent
HELPER = HERE.parent / 'bin' / 'roslyn' / 'GraphAnalyzer.dll'


def analyze(request):
    dotnet = os.environ.get('AI_ENV_DOTNET') or shutil.which('dotnet')
    if not dotnet:
        dotnet = next((str(p) for p in (Path('/usr/local/share/dotnet/dotnet'), Path('/usr/share/dotnet/dotnet')) if p.is_file()), None)
    if not dotnet:
        raise RuntimeError('C# analysis needs the .NET 8 runtime; install it or set AI_ENV_DOTNET')
    if not HELPER.is_file():
        raise RuntimeError('Roslyn helper is missing; run python3 analyzers/csharp/build.py with a local .NET 8 SDK')
    result = subprocess.run([dotnet, str(HELPER)], input=json.dumps(request, ensure_ascii=False),
                            capture_output=True, text=True, cwd=request['root'], timeout=110,
                            env={**os.environ, 'DOTNET_CLI_TELEMETRY_OPTOUT': '1', 'DOTNET_NOLOGO': '1'})
    if result.returncode:
        raise RuntimeError('Roslyn helper failed: ' + result.stderr[-4000:])
    if len(result.stdout.encode()) > 8 * 1024 * 1024:
        raise RuntimeError('Roslyn graph exceeds the 8 MiB protocol limit; select fewer source files')
    graph = json.loads(result.stdout)
    # Include the actual runtime host and wrapper, in addition to managed metadata.
    existing = {item['path'] for item in graph['inputs']}
    for path in (Path(dotnet).resolve(), Path(__file__).resolve()):
        if str(path) not in existing:
            graph['inputs'].append({'path': str(path), 'hash': hashlib.sha256(path.read_bytes()).hexdigest()})
    graph['inputs'].sort(key=lambda item: item['path'])
    return graph


def failure(error):
    return {'schema_version': 1, 'language': 'csharp',
            'analyzer': {'name': 'roslyn', 'version': 'unavailable', 'capabilities': []},
            'nodes': [], 'edges': [], 'diagnostics': [{'severity': 'error', 'path': None, 'line': 0, 'message': str(error)}],
            'unresolved': [], 'inputs': [], 'analysis_complete': False,
            'limitations': ['No lexical fallback; a working packaged Roslyn helper and .NET runtime are required.']}


if __name__ == '__main__':
    try:
        request = json.load(sys.stdin)
        response = analyze(request)
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.SubprocessError) as error:
        print(json.dumps(failure(error), ensure_ascii=False, separators=(',', ':')))
        sys.exit(1)
    print(json.dumps(response, ensure_ascii=False, separators=(',', ':')))
