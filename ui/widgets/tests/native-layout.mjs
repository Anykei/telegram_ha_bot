import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Optional macOS SDK renderer check. Replay the actual rendered DOM and CSS,
// not Chromium's computed styles: native flex must calculate its own geometry.
export async function checkNativeLayout({evaluate, doc, target, label, bottom, checkClock=true}) {
  const runner=process.env.HA_WIDGET_NATIVE_RUNNER;
  if(!runner) return;
  const snapshot=await evaluate(`(() => {
    const d=${doc}, target=${target}, ops=[], ids=new Map();let next=2;
    function visit(n,parent) {
      if(n.nodeType!==1 && n.nodeType!==3 || n.tagName==='SCRIPT') return;
      const id=next++;ids.set(n,id);
      if(n.nodeType===3) ops.push({op:'text',id,data:n.textContent});
      else {
        const tag=n.localName, adopt=['html','head','body'].includes(tag)?tag:undefined;
        ops.push({op:'el',id,tag,adopt,ns:n.namespaceURI,attrs:Array.from(n.attributes,a=>[a.name,a.value])});
        if(tag==='style') ops.push({op:'css',id,text:n.textContent});
        else for(const child of n.childNodes) visit(child,id);
      }
      if(parent) ops.push({op:'ins',id,parent});
    }
    visit(d.documentElement);
    return {ops,id:ids.get(target),width:d.documentElement.clientWidth,
      area:d.defaultView.TelegramWidget.contentSafeAreaInset};
  })()`);
  const dir=await mkdtemp(join(tmpdir(),'ha-native-layout-'));
  try {
    const file=join(dir,'messages.json');
    await writeFile(file,JSON.stringify([
      {type:'safeArea',inset:snapshot.area},
      {type:'ops',ops:snapshot.ops},
      {type:'query',qid:1,query:{q:'rect',id:snapshot.id}}
    ]));
    const output=execFileSync(runner,[file,join(dir,'screen.png'),'--width',String(snapshot.width)],{encoding:'utf8',timeout:30000});
    const rect=output.split('\n').filter(line=>line.includes('answer → '))
      .flatMap(line=>JSON.parse(line.split('answer → ')[1]))
      .find(m=>m.type==='result'&&m.qid===1)?.result;
    assert.ok(rect?.height>0,`${label}: native target visible`);
    assert.ok(Math.abs(rect.bottom-bottom)<1,`${label}: native bottom ${rect.bottom}, expected ${bottom}`);
    const a=snapshot.area;
    if(checkClock && rect.bottom>600-a.bottom) {
      assert.ok(rect.left>=a.cornerLeft && rect.right<=snapshot.width-a.cornerRight,`${label}: native controls avoid the clock`);
    }
    console.log(`Native layout passed: ${label}, bottom=${rect.bottom}.`);
  } finally { await rm(dir,{recursive:true,force:true}); }
}
