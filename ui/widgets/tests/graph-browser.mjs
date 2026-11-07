import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

export async function checkGraph({root,command,evaluate,until,screenshot}) {
  const fixtures=JSON.parse(await readFile(root+'preview/graph-fixtures.json','utf8'));
  const doc=`document.getElementById('release-frame').contentDocument`;
  const win=`document.getElementById('release-frame').contentWindow`;
  const get=id=>`${doc}.getElementById('${id}')`;
  const button=key=>`${doc}.querySelector('[data-key="${key}"]')`;
  const state=`JSON.parse(${win}.TelegramWidget.chatData)`;
  async function reload(id,{theme='light',width=360,scenario='ok'}={}) {
    await evaluate(`new Promise(resolve=>{
      document.getElementById('release-frame').addEventListener('load',()=>resolve(true),{once:true});
      theme=${JSON.stringify(theme)};width=${width};
      document.getElementById('scenario').value=${JSON.stringify(scenario)};
      document.getElementById('fixture').value=${JSON.stringify(id)};
      document.getElementById('fixture').dispatchEvent(new Event('change'));
    })`);
    await until(`${get('g-line')} && ${get('r-title')}`);
  }
  const push=patch=>evaluate(`${win}.previewSetData({...${state},...${JSON.stringify(patch)}})`);
  async function requests() {
    await evaluate(`${win}.__requests=[];${win}.TelegramWidget.request=value=>new Promise((resolve,reject)=>${win}.__requests.push({value,resolve,reject}))`);
  }
  const reply=answer=>evaluate(`${win}.__requests.at(-1).resolve({result:JSON.stringify(${JSON.stringify(answer)})})`);
  async function dimensions(label) {
    const result=await evaluate(`({height:${doc}.body.scrollHeight,width:${doc}.documentElement.scrollWidth,available:${doc}.documentElement.clientWidth,
      targets:Array.from(${doc}.querySelectorAll('button')).map(b=>b.getBoundingClientRect()).filter(r=>r.width).map(r=>({width:r.width,height:r.height}))})`);
    assert.equal(result.height,600,`${label}: fixed card height`);
    assert.ok(result.width<=result.available,`${label}: no horizontal overflow`);
    assert.ok(result.targets.every(r=>r.height>=44&&r.width>=43.9),`${label}: targets >=44px ${JSON.stringify(result.targets)}`);
    if(await evaluate(`!${get('g-box')}.classList.contains('r-hidden')`)) {
      const graph=await evaluate(`(() => {
        const stage=${get('g-stage')},empty=stage.classList.contains('r-hidden'),plot=(empty?${get('g-empty')}:${doc}.querySelector('svg')).getBoundingClientRect();
        return {height:plot.height,stage:empty?plot.height:stage.getBoundingClientRect().height};
      })()`);
      assert.ok(graph.height>=144,`${label}: readable plot`);
      assert.ok(Math.abs(graph.height-graph.stage)<1,`${label}: plot fills spare height`);
    }
    assert.equal(await evaluate(`(() => {
      const a=${win}.TelegramWidget.contentSafeAreaInset,width=${get('release')}.clientWidth;
      return !!a && Array.from(${doc}.querySelectorAll('button,.r-text,#r-feedback')).some(n=>{
        const r=n.getBoundingClientRect();
        return r.width && r.height && r.bottom>600-a.bottom+.1 &&
          ((a.cornerLeft>0 && r.left<a.cornerLeft-.1) || (a.cornerRight>0 && r.right>width-a.cornerRight+.1));
      });
    })()`),false,`${label}: controls and text avoid Telegram clock`);
  }
  async function collect(label) {
    // A narrow calendar initially opens the selected week. All other weeks remain reachable.
    while(await evaluate(`!${get('r-footer')}.classList.contains('r-hidden') && !${get('r-local-prev')}.disabled`)) await evaluate(`${get('r-local-prev')}.click()`);
    const keys=new Set();let text='',count=0;
    do {
      assert.ok(++count<=12,`${label}: finite pagination`);
      await dimensions(label);
      const value=await evaluate(`({text:Array.from(${doc}.querySelectorAll('.r-text')).map(n=>n.textContent).join(''),
        keys:Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>b.dataset.key),
        last:${get('r-footer')}.classList.contains('r-hidden') || ${get('r-local-next')}.disabled})`);
      text+=value.text;value.keys.forEach(k=>keys.add(k));if(value.last)break;
      await evaluate(`${get('r-local-next')}.click()`);
    } while(true);
    return {keys,text,count};
  }
  await command('Emulation.setDeviceMetricsOverride',{width:1280,height:1080,deviceScaleFactor:1,mobile:false});
  await command('Page.navigate',{url:pathToFileURL(root+'dist/graph-preview.html').href});
  await until(`${get('r-title')}.textContent==='Температура'`);
  for(const theme of ['light','dark']) for(const width of [200,320,360,430]) for(const f of fixtures) {
    const label=`${f.id}/${theme}/${width}`;
    await reload(f.id,{theme,width});
    assert.equal(await evaluate(`${get('r-title')}.textContent`),f.page.title,label);
    assert.equal(await evaluate(`${get('r-back')}.classList.contains('r-hidden')`),false);
    const all=await collect(label);
    f.page.b.flat().forEach(b=>assert.ok(all.keys.has(b[1]),`${label}: reachable ${b[0]}`));
    assert.ok(all.text.includes(f.page.body),`${label}: full description`);
    if(f.page.c) for(let d=1;d<=f.page.c[1];d++) assert.ok(all.keys.has('d'+d),`${label}: day ${d}`);
    if(width>=320) assert.equal(all.count,1,`${label}: one screen`);
  }
  for(const side of ['left','right']) for(const width of [200,320,360,430]) for(const f of fixtures.filter(f=>!f.id.endsWith('_en'))) {
    await reload(f.id,{width,theme:'dark'});
    await evaluate(`${win}.previewSetSafeArea(64,${side==='right'?80:0},${side==='left'?80:0})`);
    await collect(`${f.id}/clock-${side}/${width}`);
  }
  // The graph has no preset periods, shifts or server paging. Available height belongs to the plot.
  await reload('numeric',{width:360,theme:'dark'});
  await evaluate(`${win}.previewSetSafeArea(64,80)`);
  const height=await evaluate(`${doc}.querySelector('svg').getBoundingClientRect().height`);
  assert.ok(height>200, `plot expands to ${height}px`);
  assert.equal(await evaluate(`${doc}.querySelectorAll('[data-key]').length`),2);
  assert.equal(await evaluate(`${get('r-footer')}.classList.contains('r-hidden')`),true);
  const bottom=await evaluate(`${get('r-content')}.getBoundingClientRect().bottom`);
  await screenshot('graph-calendar-overview-dark');
  assert.ok(Math.abs(600-bottom-16)<1,`last control uses space beside clock: bottom=${bottom}, height=${height}, padding=${await evaluate(win+".getComputedStyle("+get('release')+").paddingBottom")}`);
  await screenshot('graph-calendar-overview-dark');
  await evaluate(`${win}.previewSetSafeArea(64)`);
  assert.ok(await evaluate(`${doc}.querySelector('svg').getBoundingClientRect().height`)<height);
  await dimensions('unknown clock reserves full band');
  for(const id of ['range','calendar','calendar_previous','calendar_six_weeks','hours','minutes']) {
    await reload(id,{width:360,theme:'dark'});
    await evaluate(`${win}.previewSetSafeArea(64,80)`);
    await screenshot('graph-'+id+'-dark');
  }
  // Calendar boundaries and selected state; disabled days cannot dispatch.
  for(const id of ['calendar','calendar_previous','calendar_leap','calendar_six_weeks']) {
    await reload(id);await requests();const f=fixtures.find(f=>f.id===id),c=f.page.c;
    if(c[2]) assert.equal(await evaluate(`${button('d'+c[2])}.getAttribute('aria-pressed')`),'true');
    for(let d=1;d<=c[1];d++) {
      const off=d<c[3]||d>c[4];assert.equal(await evaluate(`${button('d'+d)}.disabled`),off);
      if(off) await evaluate(`${button('d'+d)}.click()`);
    }
    assert.equal(await evaluate(`${win}.__requests.length`),0);
    await evaluate(`${button('d'+c[3])}.click()`);
    assert.ok((await evaluate(`${win}.__requests[0].value`)).endsWith(':d'+c[3]));
  }
  // Preview follows the server-generated route map in the same document.
  await reload('numeric');
  await evaluate(`${win}.__sameDocument=123;${button('period')}.click()`);
  await until(`${get('release')}.dataset.kind==='range' && !${get('r-read')}.disabled`);
  const range=fixtures.find(f=>f.id==='range');
  const dateKey=Object.keys(range.links).find(k=>range.links[k]==='calendar');
  await evaluate(`${button(dateKey)}.click()`);
  await until(`${get('release')}.dataset.kind==='calendar' && !${get('r-read')}.disabled`);
  const cal=fixtures.find(f=>f.id==='calendar');
  await evaluate(`${button('d'+cal.page.c[2])}.click()`);
  await until(`${get('release')}.dataset.kind==='range' && !${get('r-read')}.disabled`);
  const timeKey=Object.keys(range.links).find(k=>range.links[k]==='hours');
  await evaluate(`${button(timeKey)}.click()`);
  await until(`${get('release')}.dataset.kind==='hours' && !${get('r-read')}.disabled`);
  const hours=fixtures.find(f=>f.id==='hours');
  await evaluate(`${button(hours.page.b[2][0][1])}.click()`);
  await until(`${get('release')}.dataset.kind==='minutes' && !${get('r-read')}.disabled`);
  const minutes=fixtures.find(f=>f.id==='minutes');
  for(const label of ['05','+1','−1','Готово']) {
    const k=minutes.page.b.flat().find(b=>b[0]===label)[1];
    await evaluate(`${button(k)}.click()`);await until(`!${get('r-read')}.disabled`);
  }
  assert.equal(await evaluate(`${get('release')}.dataset.kind`),'range');
  await evaluate(`${button(range.page.b[2][0][1])}.click()`);
  await until(`${get('release')}.dataset.kind==='chart' && !${get('r-read')}.disabled`);
  assert.equal(await evaluate(`${win}.__sameDocument`),123);
  // One request in flight; timeout only permits a read, never repeats the edit.
  for(const dataFirst of [false,true]) {
    await reload('range');await requests();
    await evaluate(`${button(dateKey)}.click();${button(dateKey)}.click();${get('r-read')}.click()`);
    assert.equal(await evaluate(`${win}.__requests.length`),1);
    await evaluate(`${win}.__requests[0].reject(Error('timeout'))`);
    await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
    assert.equal(await evaluate(`${button(dateKey)}.disabled`),true);
    await evaluate(`${get('r-read')}.click()`);
    if(dataFirst) await push({seq:100});await reply({accepted:true});if(!dataFirst)await push({seq:100});
    await until(`!${button(dateKey)}.disabled`);
    await push({seq:2,title:'Stale'});
    assert.equal(await evaluate(`${get('r-title')}.textContent`),range.page.title);
  }
  // Revocation/forwarding removes private data, even if an older response arrives later.
  for(const id of ['numeric','calendar']) {
    await reload(id);await requests();await evaluate(`${get('r-back')}.click()`);
    await evaluate(`${win}.previewSetData({denied:true})`);await reply({page:{...fixtures[0].page,seq:100}});
    assert.equal(await evaluate(`${get('g-line')}.getAttribute('d')`),'');
    assert.equal(await evaluate(`${get('r-content')}.textContent`),'');
    await reload(id,{scenario:'forward'});
    assert.equal(await evaluate(`${get('r-content')}.textContent`),'');
    assert.ok(await evaluate(`${get('r-feedback')}.textContent.includes('/start')`));
  }
  const invalid=[{graph:{...fixtures[0].page.graph,plot:'A'.repeat(95)}},{graph:{...fixtures[0].page.graph,plot:'AA_'+'AAA'.repeat(31)}},
    {kind:'calendar',c:[7,31,1,1,31]},{kind:'calendar',c:[0,32,1,1,31]},{kind:'calendar',c:[0,31,1,20,10]},
    {kind:'calendar',c:[0,31,1,0,31]},{b:[[['bad','<html>','nav']]]}];
  for(const bad of invalid) {
    await reload('numeric');await requests();await push(bad);
    assert.equal(await evaluate(`${get('r-content')}.textContent`),'');
    assert.equal(await evaluate(`${get('g-line')}.getAttribute('d')`),'');
    assert.equal(await evaluate(`${win}.__requests.length`),0);
  }
  await reload('numeric');await push({graph:{...fixtures[0].page.graph,top:'<img src=x onerror=alert(1)>'}});
  assert.equal(await evaluate(`${doc}.querySelector('img')`),null);
  await command('Emulation.setDeviceMetricsOverride',{width:390,height:1100,deviceScaleFactor:1,mobile:true});
  await reload('calendar',{width:320,theme:'dark'});
  await screenshot('graph-calendar-mobile');await dimensions('calendar/mobile');
  assert.ok(await evaluate('document.documentElement.scrollWidth<=innerWidth'));
  console.log(`Graph/calendar checks passed: ${fixtures.length} fixtures × 2 themes × 4 widths; clock corners, all dates/hours/minutes, same-document editing, disabled dates, ACK races, timeout recovery, stale/denied/malformed state.`);
}
