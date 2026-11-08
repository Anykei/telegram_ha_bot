import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Optional macOS SDK renderer check. Replay the actual rendered DOM and CSS,
// not Chromium's computed styles: native flex must calculate its own geometry.
export async function checkNativeLayout({evaluate, doc, target, label, bottom, checkClock=true, hitTest=false}) {
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
    return {ops,id:ids.get(target),rect:target.getBoundingClientRect().toJSON(),
      descendants:Array.from(target.querySelectorAll('*'),n=>ids.get(n)),width:d.documentElement.clientWidth,
      area:d.defaultView.TelegramWidget.contentSafeAreaInset};
  })()`);
  const dir=await mkdtemp(join(tmpdir(),'ha-native-layout-'));
  let passed=false;
  try {
    const file=join(dir,'messages.json');
    await writeFile(file,JSON.stringify([
      {type:'safeArea',inset:snapshot.area},
      {type:'ops',ops:snapshot.ops},
      {type:'query',qid:1,query:{q:'rect',id:snapshot.id}},
      ...[snapshot.rect.top+2,snapshot.rect.bottom-2].map((y,i)=>({
        type:'query',qid:i+2,query:{q:'elementFromPoint',x:(snapshot.rect.left+snapshot.rect.right)/2,y}
      }))
    ]));
    const output=execFileSync(runner,[file,join(dir,'screen.png'),'--width',String(snapshot.width)],{encoding:'utf8',timeout:30000});
    const results=output.split('\n').filter(line=>line.includes('answer → '))
      .flatMap(line=>JSON.parse(line.split('answer → ')[1])).filter(m=>m.type==='result');
    // The SDK reports zero rects for some nested flex items although they paint
    // and receive taps. Verify both ends of those buttons by native hit testing.
    const rect=hitTest?snapshot.rect:results.find(m=>m.qid===1)?.result;
    if(hitTest) for(const qid of [2,3]) assert.ok(
      [snapshot.id,...snapshot.descendants].includes(results.find(m=>m.qid===qid)?.result),
      `${label}: native button receives taps at both vertical edges`);
    assert.ok(rect?.height>0,`${label}: native target visible: ${JSON.stringify(rect)}`);
    assert.ok(Math.abs(rect.bottom-bottom)<1,`${label}: native bottom ${rect.bottom}, expected ${bottom}`);
    const a=snapshot.area;
    if(checkClock && rect.bottom>600-a.bottom) {
      assert.ok(rect.left>=a.cornerLeft && rect.right<=snapshot.width-a.cornerRight,`${label}: native controls avoid the clock`);
    }
    console.log(`Native layout passed: ${label}, bottom=${rect.bottom}.`);
    passed=true;
  } finally {
    if(passed) await rm(dir,{recursive:true,force:true});
    else console.error(`Native layout artifacts: ${dir}`);
  }
}
