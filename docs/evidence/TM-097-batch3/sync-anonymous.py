#!/usr/bin/env python3
"""Published installed client, no inherited auth/config, followed by a strict offline gate."""
import json
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[3]
out = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix='.sync-proof-', dir=root) as directory:
    temp = Path(directory)
    user, glob = temp/'user.npmrc', temp/'global.npmrc'
    user.write_text(''); glob.write_text('')
    env = {'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8', 'NPM_CONFIG_USERCONFIG': str(user),
           'NPM_CONFIG_GLOBALCONFIG': str(glob), 'NPM_CONFIG_CACHE': str(temp/'cache')}
    cli = root/'node_modules/@bytedesk/design-client/cli.mjs'
    package = json.loads((cli.parent/'package.json').read_text())
    assert package['version'] == '2.2.1'
    for label, command in [
        ('sync-anonymous', ['/usr/bin/node', str(cli), 'sync']),
        ('sync-offline-check', ['/usr/bin/timeout', '20', '/usr/bin/unshare', '-Urn', '/usr/bin/env', '-i',
                                'PATH=/usr/bin:/bin', '/usr/bin/node', str(cli), 'sync', '--check']),
    ]:
        result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True, timeout=120)
        output = (result.stdout+result.stderr).replace(str(root), '<worktree>')
        (out/f'{label}.txt').write_text(output+f'\nexit_code={result.returncode}\n')
        print(output, flush=True)
        if result.returncode: raise SystemExit(result.returncode)
