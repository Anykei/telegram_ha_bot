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
      document.querySelectorAll('[data-theme]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.theme===theme)));
      document.querySelectorAll('[data-width]').forEach(b=>b.setAttribute('aria-pressed',String(Number(b.dataset.width)===width)));
      document.getElementById('fixture').dispatchEvent(new Event('change'));
    })`);
    await until(`${get('g-line')} && ${get('r-title')}`);
  }
  async function capture(name) {
    await until(`document.getElementById('release-frame').clientHeight === Math.min(600,Math.ceil(${get('release')}.getBoundingClientRect().height))`);
    await screenshot(name);
  }
  async function dimensions(label) {
    const result=await evaluate(`({height:${doc}.body.scrollHeight,width:${doc}.documentElement.scrollWidth,available:${doc}.documentElement.clientWidth,
      targets:Array.from(${doc}.querySelectorAll('button')).map(b=>b.getBoundingClientRect()).filter(r=>r.width).map(r=>({width:r.width,height:r.height}))})`);
    assert.equal(result.height,600,`${label}: fixed card height`);
    assert.ok(result.width<=result.available,`${label}: no overflow`);
    assert.ok(result.targets.every(r=>r.height>=44&&r.width>=44),`${label}: targets >=44px`);
  }
  async function collect(label) {
    let text='',actions=[],parts=0;
    do {
      assert.ok(++parts<=12,`${label}: finite pagination`);
      await dimensions(label);
      assert.equal(await evaluate(`${get('r-back')}.classList.contains('r-hidden')`),false);
      const value=await evaluate(`({text:Array.from(${doc}.querySelectorAll('.r-text')).map(n=>n.textContent).join(''),
        actions:Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>[b.dataset.key,b.firstChild.textContent]),
        last:${get('r-local')}.classList.contains('r-hidden') || ${get('r-local-next')}.getAttribute('aria-disabled')==='true'})`);
      text+=value.text;actions.push(...value.actions);
      if(value.last) break;
      await evaluate(`${get('r-local-next')}.click()`);
    } while(true);
    return {text,actions,parts};
  }
  async function requests() {
    await evaluate(`${win}.__requests=[];${win}.TelegramWidget.request=value=>new Promise((resolve,reject)=>${win}.__requests.push({value,resolve,reject}))`);
  }
  const reply=answer=>evaluate(`${win}.__requests.at(-1).resolve({result:JSON.stringify(${JSON.stringify(answer)})})`);
  const push=patch=>evaluate(`${win}.previewSetData({...${state},...${JSON.stringify(patch)}})`);
  await command('Emulation.setDeviceMetricsOverride',{width:1280,height:1080,deviceScaleFactor:1,mobile:false});
  await command('Page.navigate',{url:pathToFileURL(root+'dist/graph-preview.html').href});
  await until(`${get('g-line')} && ${get('r-title')}.textContent==='Температура'`);
  for(const theme of ['light','dark']) for(const width of [200,320,360,430]) for(const f of fixtures) {
    const label=`${f.id}/${theme}/${width}`;
    await reload(f.id,{theme,width});
    assert.equal(await evaluate(`${doc}.documentElement.getAttribute('data-theme')`),theme);
    assert.equal(await evaluate(`${get('r-read')}.getAttribute('aria-label')`),f.page.lang==='ru'?'Обновить':'Refresh');
    const paths=await evaluate(`({line:${get('g-line')}.getAttribute('d'),range:${get('g-range')}.getAttribute('d'),gaps:${get('g-gaps')}.getAttribute('d')})`);
    if(f.page.graph) {
      const buckets=f.page.graph.plot.match(/.../g),known=buckets.filter(t=>t!=='...').length;
      const runs=buckets.filter((t,i)=>t!=='...'&&(i===0||buckets[i-1]==='...')).length;
      assert.equal((paths.range.match(/M/g)||[]).length,known,`${label}: every known range retained`);
      assert.equal((paths.line.match(/H/g)||[]).length,known,`${label}: last value in each known bucket`);
      assert.equal((paths.line.match(/M/g)||[]).length,runs,`${label}: gaps start separate lines`);
      assert.equal((paths.gaps.match(/M/g)||[]).length,32-known,`${label}: unknown intervals shaded`);
      if(f.id.startsWith('binary')) assert.ok(paths.range.includes('136V4'),`${label}: brief full-range peak retained`);
      if(f.id.startsWith('constant')) assert.equal(new Set(paths.line.match(/V[\d.]+/g)).size,1,`${label}: constant signal stays flat`);
      assert.equal(await evaluate(`${get('g-empty')}.classList.contains('r-hidden')`),known>0);
      if(!known) assert.equal(await evaluate(`${get('g-empty')}.textContent`),f.page.lang==='ru'?'Нет данных':'No data');
      for(const field of ['top','bottom','start','end']) assert.equal(await evaluate(`${get('g-'+field)}.textContent`),f.page.graph[field]);
      if(width>=320) {
        assert.equal(await evaluate(`${get('r-local')}.classList.contains('r-hidden')`),true,`${label}: plot, Now and every primary period control fit together`);
        assert.equal(await evaluate(`${doc}.querySelectorAll('[data-key]').length`),f.page.b.flat().length);
        for(const key of ['p12','p24','p3','p7']) assert.ok(await evaluate(`${button(key)}`),`${label}: ${key} available on the first part`);
      }
    } else {
      assert.equal(await evaluate(`${get('g-box')}.classList.contains('r-hidden')`),true);
      assert.deepEqual(paths,{line:'',range:'',gaps:''});
    }
    const content=await collect(label);
    assert.equal(content.text,f.page.alert+f.page.notice+f.page.body+(f.page.hint||''),`${label}: every field retained`);
    assert.deepEqual(content.actions,f.page.b.flat().filter(b=>b[1]!==f.page.back).map(b=>[b[1],b[0]]),`${label}: every original control retained`);
  }
  for(const width of [200,360]) {
    await reload('numeric',{width});
    await evaluate(`${win}.previewSetSafeArea(64)`);
    const content=await collect(`safe-area/${width}`);
    const fixture=fixtures.find(f=>f.id==='numeric').page;
    assert.deepEqual(content.actions,fixture.b.flat().filter(b=>b[1]!==fixture.back).map(b=>[b[1],b[0]]));
    assert.equal(await evaluate(`${win}.getComputedStyle(${get('release')}).paddingBottom`),'64px');
    assert.ok(await evaluate(`Math.max(...Array.from(${doc}.querySelectorAll('button')).map(b=>b.getBoundingClientRect().bottom))<=536`));
  }
  for(const [id,theme] of [['numeric','light'],['binary','dark'],['empty','light'],['gaps','light'],['constant','dark']]) {
    await reload(id,{theme});await capture('graph-'+id+'-'+theme);
  }
  await reload('numeric_past',{width:320});await capture('graph-past-320');
  // Server-only details are readable in the same immutable graph asset.
  await reload('numeric');await evaluate(`${get('r-next')}.click()`);
  await until(`${get('r-count')}.textContent.endsWith(' 2/3') && ${get('r-read')}.getAttribute('aria-disabled')==='false'`);
  assert.equal(await evaluate(`${get('g-box')}.classList.contains('r-hidden')`),true);
  await evaluate(`${get('r-next')}.click()`);
  await until(`${get('r-count')}.textContent.endsWith(' 3/3') && ${get('r-read')}.getAttribute('aria-disabled')==='false'`);
  await capture('graph-details-light');
  await evaluate(`${get('r-prev')}.click()`);await until(`${get('r-count')}.textContent.endsWith(' 2/3') && ${get('r-read')}.getAttribute('aria-disabled')==='false'`);
  await evaluate(`${get('r-prev')}.click()`);await until(`!${get('g-box')}.classList.contains('r-hidden') && ${get('r-read')}.getAttribute('aria-disabled')==='false'`);
  // Period and range offsets update only after the server snapshot.
  await evaluate(`${button('p12')}.click()`);
  await until(`${get('g-start')}.textContent==='06.11 12:00' && ${get('r-read')}.getAttribute('aria-disabled')==='false'`);
  assert.equal(await evaluate(`${button('p12')}.getAttribute('aria-pressed')`),'true');
  await evaluate(`${button('shift')}.click()`);
  await until(`${get('g-end')}.textContent==='06.11 00:00' && ${get('r-read')}.getAttribute('aria-disabled')==='false'`);
  // Refresh after a timeout uses the same accepted + push recovery as release.
  await reload('numeric',{scenario:'error'});await evaluate(`${button('p12')}.click()`);
  await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
  assert.equal(await evaluate(`${button('p12')}.getAttribute('aria-disabled')`),'true');
  await evaluate(`${get('r-read')}.click()`);
  await until(`${button('p12')}.getAttribute('aria-disabled')==='false' && ${get('r-feedback')}.textContent===''`);
  for(const dataFirst of [false,true]) {
    await reload('numeric');await requests();
    await evaluate(`${button('p12')}.click();${button('p12')}.click();${get('r-read')}.click()`);
    assert.equal(await evaluate(`${win}.__requests.length`),1);
    await evaluate(`${win}.__requests[0].reject(Error('timeout'))`);
    await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
    await evaluate(`${get('r-read')}.click()`);
    if(dataFirst) await push({seq:100,rev:'a'.repeat(32)});
    await reply({accepted:true});
    if(!dataFirst) await push({seq:100,rev:'a'.repeat(32)});
    await until(`${button('p12')}.getAttribute('aria-disabled')==='false'`);
    await push({seq:2,title:'Stale',graph:fixtures.find(f=>f.id==='empty').page.graph});
    assert.equal(await evaluate(`${get('r-title')}.textContent`),'Температура');
    assert.ok(await evaluate(`${get('g-line')}.getAttribute('d')`));
  }
  // Back dispatches the issued key; revocation clears every private graph field.
  await reload('numeric');await requests();await evaluate(`${get('r-back')}.click()`);
  assert.ok((await evaluate(`${win}.__requests[0].value`)).endsWith(':room'));
  await evaluate(`${win}.previewSetData({denied:true})`);
  await reply({page:{...fixtures[0].page,seq:100}});
  assert.equal(await evaluate(`${get('g-line')}.getAttribute('d')`),'');
  for(const field of ['top','bottom','start','end']) assert.equal(await evaluate(`${get('g-'+field)}.textContent`),'');
  await reload('numeric',{scenario:'denied'});await evaluate(`${get('r-read')}.click()`);
  await until(`${get('r-title')}.textContent==='Доступ закрыт'`);
  assert.equal(await evaluate(`${get('g-range')}.getAttribute('d')`),'');
  await reload('numeric',{scenario:'forward'});
  assert.equal(await evaluate(`${get('g-line')}.getAttribute('d')`),'');
  assert.ok(await evaluate(`${get('r-feedback')}.textContent.includes('/classic')`));
  // Reject non-ASCII, malformed/partial gaps, invalid ordering and wrong label types.
  const invalid=[{plot:'A'.repeat(95)},{plot:'é'.repeat(96)},{plot:'A.A'+'AAA'.repeat(31)},
    {plot:'__A'+'AAA'.repeat(31)},{plot:'AA_'+'AAA'.repeat(31)},{top:42}];
  for(const bad of invalid) {
    await reload('numeric');await requests();
    await push({graph:{...fixtures[0].page.graph,...bad}});
    assert.equal(await evaluate(`${get('g-line')}.getAttribute('d')`),'');
    assert.equal(await evaluate(`${get('r-content')}.textContent`),'');
    assert.equal(await evaluate(`${win}.__requests.length`),0);
  }
  await reload('numeric');
  await push({graph:{...fixtures[0].page.graph,top:'<img src=x onerror=alert(1)>'}});
  assert.equal(await evaluate(`${doc}.querySelector('img')`),null);
  assert.equal(await evaluate(`${get('g-top')}.textContent`),'<img src=x onerror=alert(1)>');
  await command('Emulation.setDeviceMetricsOverride',{width:390,height:1100,deviceScaleFactor:1,mobile:true});
  await reload('numeric',{width:320});await evaluate(`document.getElementById('chat').scrollIntoView({block:'start'})`);
  await capture('graph-mobile');await dimensions('graph/mobile');
  assert.ok(await evaluate('document.documentElement.scrollWidth<=innerWidth'));
  console.log(`Graph checks passed: ${fixtures.length} fixtures × 2 themes × 4 widths; extrema, steps, explicit gaps, empty/constant data, all first-page period controls, graph-less details, RU/EN, back/read, ACK races, stale data, denial and malformed plots.`);
}
