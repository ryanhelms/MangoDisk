#!/usr/bin/env python3
import hashlib
import json
from pathlib import Path
import subprocess

out=Path(__file__).resolve().parent
a=json.loads((out/'before-metrics.json').read_text())
b=json.loads((out/'after-metrics.json').read_text())
report={}
for name in a:
    before=out/f'before-{name}.png'; after=out/f'after-{name}.png'
    run=subprocess.run(['compare','-metric','AE',str(before),str(after),'null:'],capture_output=True,text=True)
    if run.returncode not in (0,1): raise RuntimeError(run.stderr)
    report[name]={'changedPixels':int(run.stderr.strip()),
                  'geometryAndStyleIdentical':a[name]['styles']==b[name]['styles'],
                  'localTokensIdentical':a[name]['tokens']==b[name]['tokens'],
                  'beforeSha256':hashlib.sha256(before.read_bytes()).hexdigest(),
                  'afterSha256':hashlib.sha256(after.read_bytes()).hexdigest()}
(out/'comparison.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2))
