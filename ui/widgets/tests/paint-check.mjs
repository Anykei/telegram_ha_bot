import assert from 'node:assert/strict';

// Telegram's strict renderer permits 16 heavy paint boxes. Static dimming of a
// disabled <button> is exempt; aria-disabled alone is NOT an exemption. Count
// opacity layers at the synchronous query boundaries that flush DOM mutations.
// This supplements browser screenshots, which only see the completed task.
export async function watchPaintBudget({evaluate,doc,win,stable=false}) {
  await evaluate(`(() => {
    const d=${doc},w=${win},rect=w.Element.prototype.getBoundingClientRect;
    const sample=()=>{
      let heavy=0;
      for(const node of d.querySelectorAll('*')) {
        let displayed=true;
        for(let p=node;p;p=p.parentElement)if(w.getComputedStyle(p).display==='none'){displayed=false;break;}
        if(!displayed)continue;
        const s=w.getComputedStyle(node),exempt=node.tagName==='BUTTON'&&(node.disabled||node.matches(':active'));
        if((Number(s.opacity)<1&&!exempt)||s.filter!=='none'||s.backdropFilter!=='none')heavy++;
      }
      const roots=d.querySelectorAll('.r-root'),root=roots[roots.length-1];
      const view=JSON.stringify({
        text:Array.from(root.querySelectorAll('#r-context,#r-title,#r-content,#r-server,#r-feedback'),n=>n.textContent),
        kind:root.dataset.kind,bounds:rect.call(root).toJSON(),
        buttons:Array.from(root.querySelectorAll('button'),b=>({
          text:b===root.querySelector('#r-read')?'':b.textContent,bounds:rect.call(b).toJSON()
        }))
      });
      return {heavy,roots:roots.length,view};
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
      return {after:c.sample(),samples:c.samples};
    })()`);
    assert.equal(result.after.roots,1,`${label}: one live card after rendering`);
    for(const frame of [...result.samples,result.after])
      assert.ok(frame.heavy<=16,`${label}: Telegram paint limit exceeded: ${frame.heavy}/16`);
    if(stable)for(const frame of result.samples)
      assert.ok(frame.view===before.view||frame.view===result.after.view,`${label}: intermediate incomplete screen`);
    return result;
  };
}
