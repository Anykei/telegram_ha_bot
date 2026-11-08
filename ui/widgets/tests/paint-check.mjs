import assert from 'node:assert/strict';

// Native Telegram flushes queued mutations before synchronous layout queries.
// Ordinary screenshots only see the end of a JS task and miss these flashes.
export async function watchPaint({evaluate,doc,win}) {
  await evaluate(`(() => {
    const d=${doc},w=${win},rect=w.Element.prototype.getBoundingClientRect;
    const sample=()=>{
      const roots=d.querySelectorAll('.r-root'),root=roots[roots.length-1];
      return JSON.stringify({
        text:Array.from(root.querySelectorAll('#r-context,#r-title,#r-content,#r-server,#r-feedback'),n=>n.textContent),kind:root.dataset.kind,
        bounds:rect.call(root).toJSON(),background:w.getComputedStyle(root).backgroundColor,
        buttons:Array.from(root.querySelectorAll('button')).map(b=>({
          text:b===root.querySelector('#r-read')?'':b.textContent,bounds:rect.call(b).toJSON()
        }))
      });
    };
    const check=w.__paintCheck={active:false,samples:[],sample};
    const probe=()=>{if(check.active)check.samples.push(sample());};
    for(const property of ['clientWidth','scrollWidth','scrollHeight']) {
      const descriptor=Object.getOwnPropertyDescriptor(w.Element.prototype,property);
      Object.defineProperty(w.Element.prototype,property,{...descriptor,get(){probe();return descriptor.get.call(this);}});
    }
    w.Element.prototype.getBoundingClientRect=function(){probe();return rect.call(this);};
  })()`);
  return async (operation,label)=>{
    const before=await evaluate(`${win}.__paintCheck.sample()`);
    await evaluate(`${win}.__paintCheck.samples=[];${win}.__paintCheck.active=true`);
    await operation();
    const result=await evaluate(`(() => {
      const c=${win}.__paintCheck;c.active=false;
      return {after:c.sample(),samples:c.samples,roots:${doc}.querySelectorAll('.r-root').length};
    })()`);
    assert.equal(result.roots,1,`${label}: no temporary cards left after rendering`);
    assert.ok(result.samples.length>0,`${label}: checked native layout flush points`);
    const intermediate=result.samples.find(s=>s!==before&&s!==result.after);
    assert.equal(intermediate,undefined,`${label}: only complete old/new screens may be visible during layout; unexpected frame: ${intermediate}`);
  };
}
