#!/usr/bin/env python3
"""Actual compiled Vue UI, fixture-only IPC, supported desktop window sizes."""
import json
from pathlib import Path
import subprocess
import sys

stage = sys.argv[1]
assert stage in ('before', 'after')
out = Path(__file__).resolve().parent
prefix = ['agent-browser', '--session', 'tm097-b3-mangodisk', '--json']
def browser(*args, script=None):
    run = subprocess.run([*prefix, *args], input=script, capture_output=True, text=True, check=True)
    data = json.loads(run.stdout)
    if not data['success']: raise RuntimeError(data)
    return data['data']

browser('close')
browser('--init-script', str(out/'browser-fixture.js'), 'open', 'http://127.0.0.1:4434/')
metrics = {}
for label, width, height in [('wide', 1440, 1000), ('minimum', 1000, 700)]:
    for theme in ('light', 'dark'):
        name = f'{label}-{theme}'
        browser('set', 'viewport', str(width), str(height))
        browser('set', 'media', theme, 'reduced-motion')
        browser('open', 'http://127.0.0.1:4434/')
        browser('wait', '--text', 'Find cleanable items')
        browser('wait', '--fn', "document.body.innerText.includes('Fixture volume')")
        script = """(async()=>{
          await document.fonts.ready;
          for(const a of document.getAnimations()){try{a.finish();}catch{a.cancel();}}
          await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
          const root=getComputedStyle(document.documentElement);
          const styles={};
          for(const s of ['body','nav','h1','h2','main','button']){
            const e=document.querySelector(s); if(!e)continue;
            const b=e.getBoundingClientRect(),c=getComputedStyle(e);
            styles[s]={x:b.x,y:b.y,width:b.width,height:b.height,color:c.color,background:c.backgroundColor,
              font:c.fontFamily,fontSize:c.fontSize,padding:c.padding,borderRadius:c.borderRadius};
          }
          return {width:innerWidth,height:innerHeight,theme:document.documentElement.dataset.theme,
            skin:document.documentElement.dataset.skin,scrollWidth:document.documentElement.scrollWidth,
            scrollHeight:document.documentElement.scrollHeight,styles,
            tokens:Object.fromEntries(['--background','--foreground','--primary','--layout-page-padding-inline']
              .map(n=>[n,root.getPropertyValue(n).trim()])),
            fixture:{calls:[...window.__TM097_FIXTURE__.calls],denied:[...window.__TM097_FIXTURE__.denied]}};
        })()"""
        metrics[name] = browser('eval', '--stdin', script=script)['result']
        browser('mouse','move','0','0')
        browser('screenshot',str(out/f'{stage}-{name}.png'))
        print(f'Captured {stage} {name}', flush=True)
(out/f'{stage}-metrics.json').write_text(json.dumps(metrics,indent=2)+'\n')
