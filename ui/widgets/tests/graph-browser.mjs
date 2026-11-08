import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { checkNativeLayout } from './native-layout.mjs';
import { mediaExpression } from '../private-names.mjs';

export async function checkGraph({root,command,evaluate,until,screenshot,unified=false}) {
  const rawEvaluate=evaluate,rawUntil=until;
  evaluate=source=>rawEvaluate(mediaExpression(source));
  until=source=>rawUntil(mediaExpression(source));
  const fixtures=JSON.parse(await readFile(root+'preview/graph-fixtures.json','utf8'));
  const doc=`document.getElementById('release-frame').contentDocument`;
  const win=`document.getElementById('release-frame').contentWindow`;
  const get=id=>`${doc}.getElementById('${id}')`;
  const button=key=>`${doc}.querySelector('[data-key="${key}"]')`;
  const state=`JSON.parse(${win}.TelegramWidget.chatData)`;
  let asset;
  async function reload(id,{theme='light',width=360,scenario='ok'}={}) {
    const next=fixtures.find(f=>f.id===id).page.graph?'graph':'range';
    if(asset!==next) {
      await command('Page.navigate',{url:pathToFileURL(root+(unified?'prototype/dist/unified-preview.html':'dist/'+next+'-preview.html')).href});
      await until(`document.querySelector('#fixture option[value="${id}"]')`);asset=next;
    }
    await evaluate(`new Promise(resolve=>{
      document.getElementById('release-frame').addEventListener('load',()=>resolve(true),{once:true});
      theme=${JSON.stringify(theme)};width=${width};
      document.getElementById('scenario').value=${JSON.stringify(scenario)};
      document.getElementById('fixture').value=${JSON.stringify(id)};
      document.getElementById('fixture').dispatchEvent(new Event('change'));
    })`);
    await until(`${get('r-title')}`);
    await until(`document.getElementById('release-frame').clientHeight===Math.ceil(${get('release')}.getBoundingClientRect().height)`);
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
    const refresh=await evaluate(`(() => {
      const b=${get('r-read')},r=b.getBoundingClientRect();
      return {width:r.width,height:r.height,top:r.top,right:r.right,parentRight:b.parentElement.getBoundingClientRect().right};
    })()`);
    assert.equal(refresh.width,44,`${label}: compact refresh width`);
    assert.equal(refresh.height,44,`${label}: compact refresh height`);
    assert.ok(Math.abs(refresh.right-refresh.parentRight)<1,`${label}: refresh aligned right`);
    assert.equal(refresh.top,12,`${label}: refresh always 12px from the top, including the calendar`);
    assert.ok(Math.abs(result.available-refresh.right-12)<1,`${label}: refresh always 12px from the right`);
    const pager=await evaluate(`(() => {
      const rect=id=>${doc}.getElementById(id).getBoundingClientRect().toJSON();
      return {prev:rect('r-local-prev'),next:rect('r-local-next'),count:rect('r-local-count'),area:${win}.TelegramWidget.contentSafeAreaInset,shared:${get('release')}.classList.contains('r-corner')};
    })()`);
    if(pager.prev.width) {
      const center=(result.available+(pager.shared?pager.area.cornerLeft-pager.area.cornerRight:0))/2;
      assert.deepEqual([pager.prev.width,pager.prev.height,pager.next.width,pager.next.height],[44,44,44,44],`${label}: identical compact arrows`);
      assert.ok(Math.abs(pager.prev.left-(center-88))<1,`${label}: previous anchors compact group`);
      assert.ok(Math.abs(pager.next.right-(center+88))<1,`${label}: next anchors compact group`);
      assert.equal(pager.prev.top,pager.next.top,`${label}: arrows share a row`);
      assert.ok(Math.abs((pager.count.left+pager.count.right)/2-center)<1,`${label}: counter centered in the free width`);
      assert.ok(pager.next.bottom<=600-(pager.shared?16:Math.max(16,pager.area?.bottom||0))+.1,`${label}: arrows respect the timestamp`);
    }
    if(await evaluate(`${get('release')}.dataset.kind==='chart'`)) {
      assert.equal(await evaluate(`(() => {
        const content=${get('r-content')}.getBoundingClientRect(),column=(content.width-8)/2;
        return Array.from(${doc}.querySelectorAll('.r-row button')).every(b=>{
          const r=b.getBoundingClientRect();
          return Math.abs(r.width-column)<1 && (Math.abs(r.left-content.left)<1 || Math.abs(r.right-content.right)<1);
        });
      })()`),true,`${label}: all graph actions align to the same two columns`);
    }
    if(await evaluate(`${get('g-box')} && !${get('g-box')}.classList.contains('r-hidden')`)) {
      const graph=await evaluate(`(() => {
        const stage=${get('g-stage')},empty=stage.classList.contains('r-hidden'),plot=(empty?${get('g-empty')}:${doc}.querySelector('svg')).getBoundingClientRect();
        return {height:plot.height,stage:empty?plot.height:stage.getBoundingClientRect().height};
      })()`);
      const empty=await evaluate(`${get('g-line')}.getAttribute('d')===''`);
      assert.equal(await evaluate(`${win}.getComputedStyle(${get('g-empty')}).display==='none'`),!empty,`${label}: empty-state block must not take space beside a real plot`);
      assert.equal(await evaluate(`${win}.getComputedStyle(${get('g-stage')}).display==='none'`),empty,`${label}: empty chart hides plot stage`);
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
  await reload('numeric');
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
  assert.equal(await evaluate(`${doc}.querySelectorAll('[data-key]').length`),3);
  assert.equal(await evaluate(`${get('r-footer')}.classList.contains('r-hidden')`),true);
  const bottom=await evaluate(`${get('r-content')}.getBoundingClientRect().bottom`);
  await screenshot('graph-calendar-overview-dark');
  assert.ok(Math.abs(600-bottom-16)<1,`last control uses space beside clock: bottom=${bottom}, height=${height}, padding=${await evaluate(win+".getComputedStyle("+get('release')+").paddingBottom")}`);
  // SDK litehtml does not implement CSS grid; check its vertical flex sizing.
  // The full action grid and clock intersection are checked in dimensions().
  await checkNativeLayout({evaluate,doc,target:`${doc}.querySelector('.r-row:last-child button')`,label:'graph/PNG',bottom:584,checkClock:false});
  for(const [left,right] of [[0,80],[80,0],[0,160],[80,80],[0,200]]) {
    await evaluate(`${win}.previewSetSafeArea(64,${right},${left})`);
    await dimensions(`action grid/clock ${left}/${right}`);
  }
  await evaluate(`${win}.previewSetSafeArea(64)`);
  assert.ok(await evaluate(`${doc}.querySelector('svg').getBoundingClientRect().height`)<height);
  await dimensions('unknown clock reserves full band');
  // Long descriptions require paging even beside a clock. Re-measuring keeps
  // the compact group outside the clock and preserves the selected part.
  for(const [left,right] of [[0,80],[80,0],[80,80],[0,200]]) {
    await reload('numeric',{width:360,theme:'dark'});
    await evaluate(`${win}.previewSetSafeArea(64,${right},${left})`);
    const body='Дополнительные сведения о температуре и измерениях. '.repeat(12);
    await push({body});
    const all=await collect(`long graph/clock ${left}/${right}`);
    assert.ok(all.count>1,'long graph has local parts');
    assert.equal(all.text,body,'paging preserves the complete description');
    fixtures.find(f=>f.id==='numeric').page.b.flat().forEach(b=>assert.ok(all.keys.has(b[1])));
    const last=await evaluate(`${get('r-local-count')}.textContent`);
    await push({notice:''});
    assert.equal(await evaluate(`${get('r-local-count')}.textContent`),last,'refresh preserves the local part');
    await dimensions('long graph refreshed');
  }
  // PNG export uses the current snapshot and leaves the complete chart controls in place.
  for(const width of [200,320,360,430]) for(const id of ['numeric','numeric_en']) {
    await reload(id,{width,theme:'dark'});
    const before=await evaluate(`${state}.graph`);
    await evaluate(`${button('export')}.click();${button('export')}.click()`);
    assert.ok(await evaluate(`Array.from(${doc}.querySelectorAll('[data-key]')).every(b=>b.disabled)`));
    await until(`${state}.notice.includes('PNG')`);
    assert.deepEqual(await evaluate(`${state}.graph`),before);
    const all=await collect(`PNG sent/${id}/${width}`);
    if(width>=320) assert.equal(all.count,1,'export feedback and controls remain on one screen');
    await push({notice:'',alert:id==='numeric'?'Отправка не подтверждена. Проверьте чат.':'Upload not confirmed. Check the chat.'});
    const failed=await collect(`PNG failed/${id}/${width}`);
    if(width>=320) assert.equal(failed.count,1,'upload error and controls remain on one screen');
  }
  for(const id of ['range','calendar','calendar_previous','calendar_six_weeks','time','time_changed']) {
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
  // Opening and paging the two dropdowns is local; all exact values remain reachable.
  for(const width of [200,320,360,430]) for(const theme of ['dark','light']) {
    await reload('time',{width,theme});await requests();
    assert.equal(await evaluate(`${doc}.querySelector('.t-grid')`),null);
    for(const [i,max] of [[0,24],[1,60]]) {
      await evaluate(`${button('u'+i)}.click()`);
      assert.equal(await evaluate(`${button('u'+i)}.getAttribute('aria-expanded')`),'true');
      const all=await collect(`time/${width}/${theme}/${i}`);
      for(let n=0;n<max;n++) assert.ok(all.keys.has((i?'m':'h')+n));
      assert.equal(await evaluate(`${win}.__requests.length`),0,'opening and paging do not query bot');
      await evaluate(`${button('u'+i)}.click()`);
      assert.equal(await evaluate(`${doc}.querySelector('.t-grid')`),null);
    }
  }
  await reload('time',{theme:'dark'});await evaluate(`${win}.previewSetSafeArea(64,80)`);
  await screenshot('time-fields-dark');await evaluate(`${button('u0')}.click()`);
  await dimensions('expanded time/clock');await screenshot('time-hours-dropdown-dark');
  await evaluate(`${button('u1')}.click()`);await screenshot('time-minutes-dropdown-dark');
  for(const [left,right] of [[0,80],[80,0],[80,80],[0,200]]) {
    await evaluate(`${win}.previewSetSafeArea(64,${right},${left})`);
    const values=await collect(`minute picker/clock ${left}/${right}`);
    for(let n=0;n<60;n++) assert.ok(values.keys.has('m'+n),'All minutes remain reachable beside the clock');
    assert.equal(await evaluate(`${get('r-footer')}.getBoundingClientRect().bottom`),360-left-right>=200?584:536,'Picker reclaims the clock band only when its group fits');
    await checkNativeLayout({evaluate,doc,target:get('r-local-next'),label:`time/${left}/${right}`,bottom:360-left-right>=200?584:536});
  }
  await reload('range');
  await evaluate(`${win}.__sameDocument=123`);
  const range=fixtures.find(f=>f.id==='range');
  const dateKey=Object.keys(range.links).find(k=>range.links[k]==='calendar');
  await evaluate(`${button(dateKey)}.click()`);
  await until(`${get('release')}.dataset.kind==='calendar' && !${get('r-read')}.disabled`);
  const cal=fixtures.find(f=>f.id==='calendar');
  await evaluate(`${button('d'+cal.page.c[2])}.click()`);
  await until(`${get('release')}.dataset.kind==='range' && !${get('r-read')}.disabled`);
  const timeKey=Object.keys(range.links).find(k=>range.links[k]==='time');
  await evaluate(`${button(timeKey)}.click()`);
  await until(`${get('release')}.dataset.kind==='time' && !${get('r-read')}.disabled`);
  const time=fixtures.find(f=>f.id==='time');
  for(const [i,n] of [[0,9],[1,38]]) {
    await evaluate(`${button('u'+i)}.click();${button((i?'m':'h')+n)}.click()`);
    await until(`${get('t'+i)}.textContent===${JSON.stringify(String(n).padStart(2,'0'))} && !${get('r-read')}.disabled`);
    assert.equal(await evaluate(`${doc}.querySelector('.t-grid')`),null,'selection collapses dropdown');
  }
  await evaluate(`${button(time.page.b[0][0][1])}.click()`);
  await until(`${get('release')}.dataset.kind==='range' && !${get('r-read')}.disabled`);
  assert.equal(await evaluate(`${win}.__sameDocument`),123);
  // Animation is driven by rAF, including on clients that suppress CSS animations.
  for(const reduced of [false,true]) {
    await command('Emulation.setEmulatedMedia',{features:[{name:'prefers-reduced-motion',value:reduced?'reduce':'no-preference'}]});
    await reload('time');await requests();
    await evaluate(`(()=>{const style=${doc}.createElement('style');style.textContent='*{animation:none!important;transition:none!important}';${doc}.head.appendChild(style);${button('u0')}.click();${button('h9')}.click();${button('h9')}.click();})()`);
    assert.equal(await evaluate(`${win}.__requests.length`),1);
    assert.equal(await evaluate(`${get('t0')}.textContent`),'10','no optimistic confirmation');
    await reply({accepted:true});await push({seq:100,t:[9,37]});
    const moving=await evaluate(`${get('t0')}.style.transform`);
    assert.equal(moving==='translateY(0px)',reduced);
    await until(`${get('t0')}.style.transform==='translateY(0px)'`);
    await push({seq:2,t:[22,59]});assert.equal(await evaluate(`${get('t0')}.textContent`),'09');
    await evaluate(`${win}.previewSetData({denied:true})`);assert.equal(await evaluate(`${doc}.querySelector('.t-value')`),null);
  }
  await command('Emulation.setEmulatedMedia',{features:[]});
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
  for(const id of ['numeric','calendar','time']) {
    await reload(id);await requests();await evaluate(`${get('r-back')}.click()`);
    await evaluate(`${win}.previewSetData({denied:true})`);
    assert.equal(await evaluate(`${win}.__requests[1].value`),'r1:home');
    // Resolve the original navigation after recovery started, not the recovery request.
    await evaluate(`${win}.__requests[0].resolve({result:JSON.stringify(${JSON.stringify({page:{...fixtures.find(f=>f.id===id).page,seq:100}})})})`);
    await reply({denied:true});await evaluate(`${win}.previewSetData({denied:true})`);
    assert.equal(await evaluate(`${win}.__requests.length`),2,'One Home recovery; no late-response resurrection or loop');
    assert.ok(await evaluate(`!${get('g-line')} || ${get('g-line')}.getAttribute('d')===''`));
    assert.equal(await evaluate(`${get('r-content')}.textContent`),'');
    await reload(id,{scenario:'forward'});
    assert.equal(await evaluate(`${get('r-content')}.textContent`),'');
    assert.ok(await evaluate(`${get('r-feedback')}.textContent.includes('/start')`));
  }
  const invalid=[{graph:{...fixtures[0].page.graph,plot:'A'.repeat(95)}},{graph:{...fixtures[0].page.graph,plot:'AA_'+'AAA'.repeat(31)}},
    {kind:'calendar',c:[7,31,1,1,31]},{kind:'calendar',c:[0,32,1,1,31]},{kind:'calendar',c:[0,31,1,20,10]},
    {kind:'calendar',c:[0,31,1,0,31]},{kind:'time',t:[24,0]},{kind:'time',t:[0,60]},{kind:'time',t:[-1,0]},{kind:'time',t:[1]},{b:[[['bad','<html>','nav']]]}];
  for(const bad of invalid) {
    await reload(bad.kind?'range':'numeric');await requests();await push(bad);
    assert.equal(await evaluate(`${get('r-content')}.textContent`),'');
    assert.ok(await evaluate(`!${get('g-line')} || ${get('g-line')}.getAttribute('d')===''`));
    assert.deepEqual(await evaluate(`${win}.__requests.map(r=>r.value)`),['r1:home']);
  }
  await reload('numeric');await push({graph:{...fixtures[0].page.graph,top:'<img src=x onerror=alert(1)>'}});
  assert.equal(await evaluate(`${doc}.querySelector('img')`),null);
  await command('Emulation.setDeviceMetricsOverride',{width:390,height:1100,deviceScaleFactor:1,mobile:true});
  await reload('calendar',{width:320,theme:'dark'});
  await screenshot('graph-calendar-mobile');await dimensions('calendar/mobile');
  assert.ok(await evaluate('document.documentElement.scrollWidth<=innerWidth'));
  console.log(`Graph/calendar checks passed: ${fixtures.length} fixtures × 2 themes × 4 widths; clock corners, all dates/hours/minutes, same-document editing, disabled dates, ACK races, timeout recovery, stale/denied/malformed state.`);
}
