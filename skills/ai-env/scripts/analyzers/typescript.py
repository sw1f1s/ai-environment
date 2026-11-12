#!/usr/bin/env python3
"""Static TypeScript/JavaScript compiler graph; never execute target modules."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent


def analyze(request):
    node = os.environ.get('AI_ENV_NODE') or shutil.which('node')
    runtime_config = HERE.parent / 'bin/runtimes.json'
    runtime_used = False
    if not node and runtime_config.is_file():
        node = json.loads(runtime_config.read_text()).get('node')
        runtime_used = True
    if not node:
        local = HERE.parents[3] / '.tools/node/current/bin/node'
        if local.is_file():
            node = str(local)
    if not node:
        raise RuntimeError('TypeScript/JavaScript analysis needs Node.js; install it or set AI_ENV_NODE')
    compiler = HERE.parent / 'vendor/typescript/lib/typescript.js'
    if not compiler.is_file():
        raise RuntimeError('Packaged TypeScript compiler is missing; rebuild the AI Env skill bundle')
    result = subprocess.run([node, str(HERE / 'typescript.cjs')],
                            input=json.dumps(request, ensure_ascii=False), capture_output=True,
                            text=True, cwd=request['root'], timeout=110,
                            env={key: value for key, value in os.environ.items() if key not in ('NODE_OPTIONS', 'NODE_PATH')})
    if result.returncode:
        raise RuntimeError('TypeScript compiler helper failed: ' + result.stderr[-4000:])
    if len(result.stdout.encode()) > 8 * 1024 * 1024:
        raise RuntimeError('TypeScript graph exceeds 8 MiB; select fewer source files')
    graph = json.loads(result.stdout)
    paths = [Path(node).resolve(), Path(__file__).resolve()]
    if runtime_used:
        paths.append(runtime_config.resolve())
    for path in paths:
        graph['inputs'].append({'path': str(path), 'hash': hashlib.sha256(path.read_bytes()).hexdigest()})
    graph['inputs'].sort(key=lambda item: item['path'])
    return graph


if __name__ == '__main__':
    try:
        print(json.dumps(analyze(json.load(sys.stdin)), ensure_ascii=False, separators=(',', ':')))
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
