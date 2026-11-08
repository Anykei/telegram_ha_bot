import {watchPaint} from './paint-check.mjs';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { releaseExpression } from '../private-names.mjs';
import { checkNativeLayout } from './native-layout.mjs';

export async function checkRelease({root, command, evaluate, until, screenshot, unified=false}) {
  const rawEvaluate = evaluate, rawUntil = until;
  evaluate = source => rawEvaluate(releaseExpression(source));
  until = source => rawUntil(releaseExpression(source));
  const fixtures = JSON.parse(await readFile(root + 'preview/release-fixtures.json', 'utf8'));
  const rows = page => page.b ? page.b.map(row=>row.map(([text,key,kind])=>({text,key,kind}))) : page.rows;
  const visibleActions = page => rows(page).flat().filter(b=>b.key!==page.back).map(b=>[b.key,b.text]);
  const doc = `document.getElementById('release-frame').contentDocument`;
  const win = `document.getElementById('release-frame').contentWindow`;
  const get = id => `${doc}.getElementById('${({'r-local':'r-server','r-local-prev':'r-prev','r-local-next':'r-next','r-local-count':'r-count'})[id] || id}')`;
  const button = key => `${doc}.querySelector('[data-key="${key}"]')`;
  const state = `JSON.parse(${win}.TelegramWidget.chatData)`;
  const click = key => evaluate(`${button(key)}.click()`);
  const disabled = key => `${button(key)}.getAttribute('aria-disabled')`;
  async function capture(name) {
    await until(`document.getElementById('release-frame').clientHeight === Math.min(600,Math.ceil(${get('release')}.getBoundingClientRect().height))`);
    await screenshot(name);
  }
  async function reload(id, {theme = 'light', width = 360, scenario = 'ok'} = {}) {
    await evaluate(`new Promise(resolve => {
      const f=document.getElementById('release-frame');
      f.addEventListener('load',()=>resolve(true),{once:true});
      theme=${JSON.stringify(theme)};width=${width};
      document.getElementById('scenario').value=${JSON.stringify(scenario)};
      document.getElementById('fixture').value=${JSON.stringify(id)};
      document.querySelectorAll('[data-theme]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.theme===theme)));
      document.querySelectorAll('[data-width]').forEach(b=>b.setAttribute('aria-pressed',String(Number(b.dataset.width)===width)));
      document.getElementById('fixture').dispatchEvent(new Event('change'));
    })`);
    try {
      await until(`${get('release')} && ${get('r-feedback')} && ${doc}.documentElement.getAttribute('data-theme') === ${JSON.stringify(theme)}`);
    } catch (error) { throw Error(`${id}/${theme}/${width}: widget did not initialize`, {cause:error}); }
  }
  async function dimensions(label) {
    const result = await evaluate(`({
      height:${doc}.body.scrollHeight,
      width:${doc}.documentElement.scrollWidth,
      available:${doc}.documentElement.clientWidth,
      textFits:Array.from(${doc}.querySelectorAll('.r-text,.r-status')).every(p=>p.scrollWidth<=p.clientWidth),
      labelsFit:Array.from(${doc}.querySelectorAll('.r-bounded .r-row .r-button span')).every(p=>p.getBoundingClientRect().height<=2*parseFloat(${win}.getComputedStyle(p).lineHeight)+1),
      rowsFit:Array.from(${doc}.querySelectorAll('.r-bounded:not([data-kind="home"]) .r-row')).every(p=>Math.abs(p.getBoundingClientRect().height-64)<1),
      targets:Array.from(${doc}.querySelectorAll('button')).map(b=>b.getBoundingClientRect()).filter(r=>r.width).map(r=>({width:r.width,height:r.height}))
    })`);
    assert.equal(result.height, 600, `${label}: fixed card height`);
    assert.ok(result.width <= result.available, `${label}: no horizontal overflow`);
    assert.ok(result.textFits, `${label}: full text wraps without clipping`);
    assert.ok(result.labelsFit, `${label}: ordinary labels use at most two authored lines`);
    assert.ok(result.rowsFit, `${label}: ordinary rows retain 64px height`);
    assert.ok(result.targets.every(r => r.height >= 44 && r.width >= 44), `${label}: touch targets >=44px`);
    const refresh = await evaluate(`(() => {
      const b=${get('r-read')},r=b.getBoundingClientRect();
      return {width:r.width,height:r.height,top:r.top,right:r.right,parentRight:b.parentElement.getBoundingClientRect().right};
    })()`);
    assert.equal(refresh.width,44,`${label}: compact refresh width with or without pagination`);
    assert.equal(refresh.height,44,`${label}: compact refresh height`);
    assert.ok(Math.abs(refresh.right-refresh.parentRight)<1,`${label}: refresh aligned right`);
    assert.equal(refresh.top,12,`${label}: refresh always in the header`);
    assert.ok(Math.abs(result.available-refresh.right-12)<1,`${label}: refresh always 12px from the card edge, including clock insets`);
    const pager=await evaluate(`({prev:${get('r-prev')}.getBoundingClientRect().toJSON(),next:${get('r-next')}.getBoundingClientRect().toJSON(),count:${get('r-count')}.getBoundingClientRect().toJSON(),area:${win}.TelegramWidget.contentSafeAreaInset,shared:${get('release')}.classList.contains('r-corner')})`);
    if(pager.prev.width) {
      const center=(result.available+(pager.shared?pager.area.cornerLeft-pager.area.cornerRight:0))/2;
      assert.deepEqual([pager.prev.width,pager.prev.height,pager.next.width,pager.next.height],[44,44,44,44],`${label}: identical compact arrows`);
      assert.ok(Math.abs(pager.prev.left-(center-88))<1,`${label}: previous anchors compact group`);
      assert.ok(Math.abs(pager.next.right-(center+88))<1,`${label}: next anchors compact group`);
      assert.equal(pager.prev.top,pager.next.top,`${label}: arrows share a row`);
      assert.ok(Math.abs((pager.count.left+pager.count.right)/2-center)<1,`${label}: counter centered in the free width`);
      assert.ok(pager.next.bottom<=600-(pager.shared?16:Math.max(16,pager.area?.bottom||0))+.1,`${label}: arrows respect the timestamp`);
    }
    if(await evaluate(`${get('release')}.dataset.kind==='home'`)) {
      assert.equal(await evaluate(`(()=>{const h=${get('r-title')}.getBoundingClientRect(),r=${get('r-read')}.getBoundingClientRect();return h.top===r.top&&h.right<=r.left-8})()`),true,`${label}: Home title shares the header without overlapping refresh`);
    }
    const overlap = await evaluate(`(() => {
      const a=${win}.TelegramWidget.contentSafeAreaInset, width=${get('release')}.clientWidth;
      if(!a) return false;
      return Array.from(${doc}.querySelectorAll('button,.r-text,.r-status,#r-feedback')).some(n=>{
        const r=n.getBoundingClientRect();
        return r.width && r.height && r.bottom>600-a.bottom+.1 &&
          ((a.cornerLeft>0 && r.left<a.cornerLeft-.1) || (a.cornerRight>0 && r.right>width-a.cornerRight+.1));
      });
    })()`);
    assert.equal(overlap,false,`${label}: controls and text stay outside the native time corner`);
  }
  async function collectParts(label, page) {
    let text = '', actions = [], status = [], part = 0;
    const overflow = await evaluate(`({title:${get('r-title')}.scrollWidth>${get('r-title')}.clientWidth,context:${get('r-context')}.scrollWidth>${get('r-context')}.clientWidth})`);
    const expected = (overflow.context?page.context:'')+(overflow.title?page.title:'')+page.alert+page.notice+page.body+(page.hint||'');
    while (true) {
      assert.ok(++part <= 40, `${label}: finite local pagination`);
      await dimensions(`${label}/part${part}`);
      if (page && page.back) {
        const back = await evaluate(`({text:${get('r-back')}.textContent,rect:${get('r-back')}.getBoundingClientRect().toJSON()})`);
        const cancel=['form','confirmation'].includes(page.kind);
        assert.equal(back.text, '‹ '+(page.lang === 'ru' ? cancel?'Отмена':'Назад' : cancel?'Cancel':'Back'));
        assert.ok(back.rect.width >= 44 && back.rect.height >= 44 && back.rect.top < 30, `${label}/part${part}: persistent Back at the top`);
      }
      const content = await evaluate(`({
        text:Array.from(${doc}.querySelectorAll('.r-text')).map(n=>n.textContent).join(''),
        actions:Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>[b.dataset.key,b.firstChild.textContent]),
        status:Array.from(${doc}.querySelectorAll('div.r-status')).map(s=>Array.from(s.children,n=>n.textContent))
      })`);
      text += content.text; actions.push(...content.actions); status.push(...content.status);
      if (text===expected && actions.length===visibleActions(page).length && status.length===(page.status||[]).length) break;
      assert.notEqual(await evaluate(`${get('r-next')}.getAttribute('aria-disabled')`),'true',
        `${label}: content remains reachable; ${JSON.stringify({text, expected, actions, expectedActions:visibleActions(page), status, part})}`);
      await evaluate(`${get('r-local-next')}.click()`);
    }
    for (let i = 1; i < part; i++) await evaluate(`${get('r-local-prev')}.click()`);
    return {text, actions, status, parts: part};
  }
  async function serverStep(direction='next') {
    for(let part=0;part<40;part++) {
      await evaluate(`${get('r-'+direction)}.click()`);
      if(await evaluate(`${get('r-read')}.getAttribute('aria-disabled')==='true'`)) {
        await until(`${get('r-read')}.getAttribute('aria-disabled')==='false'`); return;
      }
    }
    assert.fail('Server page remains reachable through the unified pager');
  }
  async function assertPreserved(page, label) {
    const content = await collectParts(label, page);
    assert.ok(content.text.endsWith(page.alert + page.notice + page.body + (page.hint || '')), `${label}: all body/hint text survives local pagination exactly`);
    assert.deepEqual(content.actions, visibleActions(page), `${label}: every original action survives; duplicate Back is represented by the pinned control`);
    assert.deepEqual(content.status, page.status || [], `${label}: every status field survives local pagination`);
    assert.equal(await evaluate(`${get('r-title')}.textContent`), page.title);
    assert.equal(await evaluate(`${get('release')}.getAttribute('data-kind')`), page.kind || 'menu');
    assert.equal(await evaluate(`${get('r-read')}.getAttribute('aria-label')`), page.lang === 'ru' ? 'Обновить' : 'Refresh');
    return content;
  }
  async function controlRequests() {
    await evaluate(`{
      const w=${win};w.__requests=[];
      w.TelegramWidget.request=value=>new Promise((resolve,reject)=>w.__requests.push({value,resolve,reject}));
    }`);
  }
  const reply = value => evaluate(`${win}.__requests.at(-1).resolve({result:JSON.stringify(${JSON.stringify(value)})})`);
  const push = patch => evaluate(`${win}.previewSetData({...${state},...${JSON.stringify(patch)}})`);
  async function failAction() {
    await controlRequests(); await click('rooms');
    await evaluate(`${win}.__requests.at(-1).reject(Error('timeout'))`);
    await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
    assert.equal(await evaluate(disabled('rooms')), 'true');
  }
  await command('Emulation.setDeviceMetricsOverride', {width:1280,height:1080,deviceScaleFactor:1,mobile:false});
  await command('Emulation.setEmulatedMedia', {features:[{name:'prefers-reduced-motion',value:'no-preference'}]});
  await command('Page.navigate', {url:pathToFileURL(root+(unified?'prototype/dist/unified-preview.html':'dist/release-preview.html')).href});
  await until(`${get('r-title')} && ${get('r-title')}.textContent === 'Мой дом'`);
  const startupProbe=await command('Page.addScriptToEvaluateOnNewDocument',{source:`
    for(const property of ['clientWidth','scrollWidth','scrollHeight']) {
      const descriptor=Object.getOwnPropertyDescriptor(Element.prototype,property);
      Object.defineProperty(Element.prototype,property,{...descriptor,get(){
        if(window.TelegramWidget&&!window.__firstWidgetTheme)
          window.__firstWidgetTheme={actual:document.documentElement.dataset.theme,expected:window.TelegramWidget.colorScheme};
        return descriptor.get.call(this);
      }});
    }
  `});
  for(const theme of ['dark','light']) {
    await reload('room',{theme});
    const first=await evaluate(`${win}.__firstWidgetTheme`);
    assert.equal(first?.actual,theme,'Telegram theme applies before the first layout query');
  }
  await command('Page.removeScriptToEvaluateOnNewDocument',{identifier:startupProbe.identifier});
  let localPaginationSeen = false;
  for (const theme of ['light', 'dark']) {
    for (const width of [200, 320, 360, 430]) {
      for (const fixture of fixtures) {
        await reload(fixture.id, {theme, width});
        assert.equal(await evaluate(`${doc}.documentElement.getAttribute('data-theme')`), theme, `${fixture.id}/${theme}/${width}: widget initialized`);
        if (fixture.page.b && (['device','climate'].includes(fixture.page.kind) || fixture.home?.primary) && fixture.page.page===1 && width>=320) {
          assert.deepEqual(await evaluate(`Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>[b.dataset.key,b.firstChild.textContent])`), visibleActions(fixture.page), `${fixture.id}/${theme}/${width}: all primary controls on the first local part`);
          assert.ok(await evaluate(`${get('r-count')}.textContent.endsWith('.1')`));
          assert.equal(await evaluate(`${get('r-feedback')}.getBoundingClientRect().height`), 0, 'Idle feedback takes no space');
        }
        const content = await assertPreserved(fixture.page, `${fixture.id}/${theme}/${width}`);
        localPaginationSeen ||= content.parts > 1;
      }
    }
  }
  assert.ok(localPaginationSeen, 'Long fixture exercises local pagination');
  // One set of arrows traverses local parts, server chunks and source pages.
  // Previous source keys return the last server chunk, then the last local part.
  for(const lang of ['ru','en']) for(const theme of ['light','dark']) for(const width of [200,320,360,430]) {
    await reload('list_1_1'+(lang==='en'?'_en':''),{theme,width});
    await evaluate(`${win}.previewSetSafeArea(64)`);
    await evaluate(`{const w=${win},request=w.TelegramWidget.request.bind(w.TelegramWidget),timeout=w.setTimeout.bind(w);w.__requests=[];w.setTimeout=(fn,ms,...args)=>timeout(fn,ms===250?0:ms,...args);w.TelegramWidget.request=raw=>{w.__requests.push(raw);return request(raw);};}`);
    const parts=[];let steps=0;
    while(true) {
      assert.ok(++steps<40,'Finite forward traversal of 21 items');
      await dimensions(`source/${lang}/${theme}/${width}/${steps}`);
      assert.equal(await evaluate(`${doc}.querySelectorAll('.r-footer.r-pager').length`),1);
      const part=await evaluate(`({keys:Array.from(${doc}.querySelectorAll('[data-key]'),b=>b.dataset.key),label:${get('r-count')}.textContent})`);
      assert.ok(part.label.startsWith(lang==='ru'?'Список ':'List '),'Source position is explicit');parts.push(part);
      if(await evaluate(`${get('r-next')}.getAttribute('aria-disabled')==='true'`)) break;
      await evaluate(`${get('r-next')}.click()`);
      if(await evaluate(`${get('r-read')}.getAttribute('aria-disabled')==='true'`)) await until(`${get('r-read')}.getAttribute('aria-disabled')==='false'`);
    }
    assert.deepEqual(parts.flatMap(p=>p.keys),Array.from({length:21},(_,i)=>'item'+(i+1)),'All 21 items appear exactly once');
    assert.equal(await evaluate(`${win}.__requests.length`),4,'Only wire/source boundaries request the server');
    for(const expected of parts.slice(0,-1).reverse()) {
      await evaluate(`${get('r-prev')}.click()`);
      if(await evaluate(`${get('r-read')}.getAttribute('aria-disabled')==='true'`)) await until(`${get('r-read')}.getAttribute('aria-disabled')==='false'`);
      assert.deepEqual(await evaluate(`({keys:Array.from(${doc}.querySelectorAll('[data-key]'),b=>b.dataset.key),label:${get('r-count')}.textContent})`),expected,'Backwards traversal returns the exact previous part');
    }
    assert.equal(await evaluate(`${win}.__requests.length`),8);
    assert.equal(await evaluate(`${get('r-prev')}.getAttribute('aria-disabled')`),'true');
    if(lang==='ru' && theme==='light' && width===320) await capture('release-list-unified');
  }
  for(const order of ['ack-first','data-first']) {
    const target=fixtures.find(f=>f.id==='list_2_2').page;
    await reload('list_2_2',{width:200});await evaluate(`${win}.previewSetSafeArea(64)`);
    const {parts}=await collectParts('source return target',target);
    for(let i=1;i<parts;i++)await evaluate(`${get('r-next')}.click()`);
    const last=await evaluate(`${get('r-content')}.textContent`);
    await reload('list_3_1',{width:200});await evaluate(`${win}.previewSetSafeArea(64)`);await controlRequests();
    await evaluate(`${get('r-prev')}.click();${get('r-prev')}.click()`);
    assert.equal(await evaluate(`${win}.__requests.length`),1,'Source Back deduplicates requests');
    await push({seq:0}); // Stale pushes cannot consume the requested end position.
    await push({seq:2,rev:'b'.repeat(32)}); // Same-screen refresh before navigation.
    if(order==='ack-first')await reply({accepted:true});
    await evaluate(`${win}.previewSetData(${JSON.stringify({...target,seq:3,rev:'c'.repeat(32)})})`);
    if(order==='data-first')await reply({accepted:true});
    await until(`${get('r-read')}.getAttribute('aria-disabled')==='false'`);
    assert.equal(await evaluate(`${get('r-content')}.textContent`),last,'Source Back opens the last local part despite refresh/ACK ordering');
  }
  await reload('bounded_names',{width:200});
  assert.ok(await evaluate(`${button('negative')}.firstChild.textContent.startsWith('Не включать')`),'The complete negated command is retained before the object');
  await click('bounded_detail');
  await until(`${get('r-title')}.textContent==='Гостевая спальня на втором этаже рядом с лестницей'`);
  await assertPreserved(fixtures.find(f=>f.id==='bounded_detail').page,'Complete name after opening its card');
  // A compact group uses the free side of the clock band when it fits.
  for (const fixture of fixtures) for (const side of ['left','right']) {
    await reload(fixture.id,{width:360,theme:'dark'});
    await evaluate(`${win}.previewSetSafeArea(64,${side==='right'?80:0},${side==='left'?80:0})`);
    await assertPreserved(fixture.page,`${fixture.id}/corner-${side}`);
    const footer = await evaluate(`${doc}.querySelector('.r-footer').getBoundingClientRect().toJSON()`);
    const shared=await evaluate(`${get('release')}.classList.contains('r-corner')`);
    assert.ok(footer.bottom<=(shared?584.1:536.1),`${fixture.id}: footer respects the clock`);
    if(shared) assert.ok(footer.bottom>536,`${fixture.id}: pager actually uses the clock band`);
  }
  for (const width of [320,360,390,430]) {
    await reload('rooms_six',{width,theme:'dark'});
    await evaluate(`${win}.previewSetSafeArea(64,64)`);
    assert.equal(await evaluate(`${doc}.querySelectorAll('.r-row').length`),6,'All six rooms fit on the first screen');
    assert.equal(await evaluate(`${get('r-server')}.hidden`),true,'Six rooms need no extra part');
    await dimensions(`six rooms/${width}`);
    const lastRoom=button('room5');
    await checkNativeLayout({evaluate,doc,target:lastRoom,label:`six rooms/${width}`,bottom:await evaluate(`${lastRoom}.getBoundingClientRect().bottom`),hitTest:true});
    if(width===390) await capture('release-six-rooms');
    await reload('room_paginated',{width,theme:'dark'});
    const page=structuredClone(fixtures.find(f=>f.id==='room_paginated').page);
    page.b.push([['Розетка\nВЫКЛ','extra_socket','nav']]);page.seq=100;
    await push(page);
    await evaluate(`${win}.previewSetSafeArea(64,64)`);
    assert.equal(await evaluate(`${get('release')}.classList.contains('r-corner')`),true,'Seven-row room must reclaim the clock band after establishing its pager');
    assert.equal(await evaluate(`${get('r-server')}.getBoundingClientRect().bottom`),584,'Room pager uses the free clock corner');
    await checkNativeLayout({evaluate,doc,target:get('r-next'),label:`room/${width}`,bottom:584});
    assert.equal(await evaluate(`${doc}.querySelectorAll('.r-row').length`),6,'Six full-size rows fit beside the clock');
    await assertPreserved(page,`seven rows/header/${width}`);
    await dimensions(`seven rows/corner/${width}`);
    if(width===390) {
      await evaluate(`{const clock=${doc}.createElement('div');clock.textContent='07:41';clock.style.cssText='position:absolute;bottom:4px;right:4px;padding:4px 8px;border-radius:14px;background:#101b22;color:white';${doc}.body.appendChild(clock);}`);
      await capture('release-room-six-rows-clock');
    }
    // More content, an error, or widened metadata must repaginate without losing
    // a command, overlapping the clock or making the card taller.
    await reload('room_paginated',{width});
    await evaluate(`${win}.previewSetSafeArea(64,64)`);
    const seven=structuredClone(fixtures.find(f=>f.id==='room_paginated').page);
    seven.b.push([['Розетка\nВЫКЛ','extra_socket','nav']]);seven.seq=100;
    await push(seven);
    assert.equal(await evaluate(`${doc}.querySelectorAll('.r-row').length`),6);
    assert.equal((await assertPreserved(seven,`seven rows/${width}`)).parts,2);
    await push({seq:101,alert:'Нет ответа. Обновите состояние.'});
    await assertPreserved({...seven,alert:'Нет ответа. Обновите состояние.'},`error/corner/${width}`);
    await evaluate(`${win}.previewSetSafeArea(64,200)`);
    await dimensions(`wide clock/${width}`);
  }
  await reload('room_paginated',{width:200});
  await evaluate(`${win}.previewSetSafeArea(64,64)`);
  await assertPreserved(fixtures.find(f=>f.id==='room_paginated').page,'narrow/corner');
  await reload('room_paginated',{width:360,theme:'dark'});
  const clockPage=structuredClone(fixtures.find(f=>f.id==='room_paginated').page);
  clockPage.b.push([['Розетка','extra_socket','nav']]);clockPage.seq=100;
  await push(clockPage);
  await evaluate(`${win}.previewSetSafeArea(64,80);${get('r-next')}.click()`);
  const lastRoomPart=await evaluate(`${get('r-content')}.textContent`);
  for(const [left,right] of [[80,0],[80,80],[0,200],[0,80]]) {
    await evaluate(`${win}.previewSetSafeArea(64,${right},${left})`);
    // A wider clock can reduce the first part from six to five rows. The last
    // part then gains a preceding row, but the selected end of the list stays.
    assert.ok(await evaluate(`${get('r-content')}.textContent.endsWith(${JSON.stringify(lastRoomPart)})`),'Clock changes preserve the selected end of the list');
    assert.equal(await evaluate(`${get('r-count')}.textContent`),'1.2','Clock changes keep the selected part');
    await dimensions(`room part/clock ${left}/${right}`);
    await checkNativeLayout({evaluate,doc,target:get('r-next'),label:`room/clock/${left}/${right}`,bottom:360-left-right>=200?584:536});
  }
  // Media stays in Telegram attachments; the camera card preserves its controls
  // and navigation even at the client's largest reserved timestamp inset.
  for (const fixture of fixtures.filter(f=>f.camera)) {
    for (const width of [320,360,430]) {
      await reload(fixture.id,{width,theme:'dark'});
      await evaluate(`${win}.previewSetSafeArea(64)`);
      const visible=await evaluate(`Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>b.dataset.key)`);
      const primary=rows(fixture.page).flat().filter(b=>b.kind==='action').map(b=>b.key);
      for (const key of primary) assert.ok(visible.includes(key),`${fixture.id}/${width}: media command ${key} fits first part`);
      if (fixture.id==='camera' || fixture.id==='camera_en') {
        for (const key of ['a5','a6']) assert.ok(visible.includes(key),`${fixture.id}/${width}: archive and details fit first part`);
      }
      await assertPreserved(fixture.page,`${fixture.id}/safe64/${width}`);
      await controlRequests();
      await click('a0'); await click('a0');
      assert.equal(await evaluate(`${win}.__requests.length`),1,'Repeated snapshot tap is blocked while the request is pending');
      assert.equal(await evaluate(disabled('a0')),'true');
      await reply({page:{...fixture.page,seq:100,rev:'b'.repeat(32)}});
      await until(`${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
      assert.equal(await evaluate(disabled('a0')),'false');
      await dimensions(`${fixture.id}/after-snapshot/${width}`);

      for(const [left,right] of [[0,72],[72,0]]) {
        await reload(fixture.id,{width,theme:'dark'});
        await evaluate(`${win}.previewSetSafeArea(64,${left},${right})`);
        assert.deepEqual(await evaluate(`Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>[b.dataset.key,b.textContent])`),visibleActions(fixture.page),`${fixture.id}/${width}: all camera actions fit beside the clock`);
        const geometry=()=>evaluate(`(() => {
          const rows=Array.from(${doc}.querySelectorAll('.r-row'),n=>n.getBoundingClientRect().toJSON());
          return {rows,gap:${get('r-server')}.getBoundingClientRect().top-rows.at(-1).bottom};
        })()`);
        const before=await geometry();
        assert.ok(Math.abs(before.gap-8)<1,`${fixture.id}/${width}: camera rows fill the space above navigation, gap=${before.gap}`);
        await controlRequests();await click('a1');await click('a1');
        assert.equal(await evaluate(`${win}.__requests.length`),1,'Camera clip sends once while pending');
        assert.deepEqual(await geometry(),before,'Pending clip keeps the camera layout stable');
        await reply({page:{...fixture.page,seq:100,rev:'b'.repeat(32)}});
        await until(`${get('r-read')}.getAttribute('aria-disabled')==='false'`);
        assert.deepEqual(await geometry(),before,'Completed clip keeps the camera layout stable');
        await dimensions(`${fixture.id}/${width}/clock ${left}/${right}`);
      }
      if (fixture.id==='camera' && width===360) await capture('release-camera-dark-360');
    }
  }
  await reload('six_rows', {width:390});
  await evaluate(`${win}.previewSetSafeArea(24)`);
  assert.equal(await evaluate(`${doc}.querySelectorAll('.r-row').length`), 6, 'Six stable 68px rows fit when no footer is needed');
  assert.equal((await assertPreserved(fixtures.find(f=>f.id==='six_rows').page,'six bounded rows')).parts,1);
  assert.equal(await evaluate(`${get('r-server')}.getBoundingClientRect().height`),0,'Hidden pagination reserves no height');
  await dimensions('six rows');
  // Authored two-line labels make every device row predictable, even with long
  // names, duplicate aliases and custom states. Native clock space stays clear.
  for (const width of [390, 430]) {
    await reload('room_five_rows', {theme:'dark', width});
    await evaluate(`${win}.previewSetSafeArea(24)`);
    assert.equal(await evaluate(`${doc}.querySelectorAll('.r-row').length`), 5, 'Five room rows fit on a phone');
    assert.ok(await evaluate(`${get('r-count')}.textContent.endsWith('.1')`), 'No extra local part on a phone');
    const footer = await evaluate(`(() => {
      const read=${get('r-read')}.getBoundingClientRect(), pager=${get('r-server')}.getBoundingClientRect();
      return {readTop:read.top,bottom:pager.bottom,label:${get('r-read')}.getAttribute('aria-label')};
    })()`);
    assert.equal(footer.readTop,12, 'Refresh stays in the header while paging stays below content');
    assert.ok(footer.bottom <= 576, 'Controls leave native message metadata clear');
    assert.ok(footer.bottom >= 500, 'Room controls use the available card height');
    assert.equal(footer.label, 'Обновить');
    await dimensions('room/five/'+width);
    if (width === 390) await capture('release-room-five-rows-phone');
  }
  for (const width of [200, 320, 390, 430]) {
    await reload('room_long_labels', {theme:'dark', width});
    const labels = await evaluate(`Array.from(${doc}.querySelectorAll('.r-button'), b => {
      const span=b.firstChild, style=${win}.getComputedStyle(span);
      return {height:b.getBoundingClientRect().height, text:span.textContent,
        labelHeight:span.getBoundingClientRect().height,lineHeight:parseFloat(style.lineHeight),
        overflow:style.overflow,ellipsis:style.textOverflow,wrap:style.whiteSpace};
    })`);
    assert.ok(labels.length >= 5);
    for (const label of labels) {
      assert.equal(label.height, 64, 'Every room button has the same fixed height');
      assert.equal(label.text.split('\n').length, 2, 'Name and state stay on separate authored lines');
      assert.ok(label.labelHeight <= label.lineHeight * 2 + 1, 'Visible label never exceeds two lines');
      assert.equal(label.wrap, 'pre');
      assert.equal(label.overflow, 'hidden');
      assert.equal(label.ellipsis, 'ellipsis');
    }
    await assertPreserved(fixtures.find(f=>f.id==='room_long_labels').page, 'room labels/'+width);
    if (width === 390) await capture('release-room-long-labels-phone');
  }
  // A longer source list must not lose its fifth row just because local paging
  // appears. Local arrows stay in the footer; refresh remains in the header.
  for (const width of [320, 390, 430]) for (const safe of [24, 64]) {
    await reload('room_paginated', {theme:'dark', width});
    const page=structuredClone(fixtures.find(f=>f.id==='room_paginated').page);
    page.b.push([['Розетка\nВЫКЛ','extra_socket','nav']]);page.seq=100;
    await push(page);
    // Telegram's UA stylesheet applies its own body bottom padding. Our root
    // handles safe area, so body must reset it rather than reserve it twice.
    await evaluate(`{const s=${doc}.createElement('style');s.textContent=':where(body){padding-bottom:${safe}px}';${doc}.head.appendChild(s);}`);
    await evaluate(`${win}.previewSetSafeArea(${safe})`);
    assert.equal(await evaluate(`${win}.getComputedStyle(${doc}.body).paddingBottom`), '0px');
    assert.equal(await evaluate(`${doc}.querySelectorAll('.r-row').length`), 5);
    assert.equal(await evaluate(`${get('r-local')}.hidden`), false);
    assert.equal(await evaluate(`${doc}.querySelectorAll('.r-footer.r-pager').length`), 1, 'A single pager represents all continuation');
    const footer = await evaluate(`({local:${get('r-local')}.getBoundingClientRect().toJSON(),read:${get('r-read')}.getBoundingClientRect().toJSON()})`);
    assert.equal(footer.read.top,12,'Refresh does not move when local pagination appears');
    assert.ok(footer.local.top>=footer.read.bottom,'Navigation stays below the header');
    assert.ok(footer.local.bottom <= 600-safe, 'Footer leaves native metadata clear');
    const content=await assertPreserved(page,'room local paging/'+width);
    assert.equal(content.parts, 2);
    if (width === 390 && safe === 64) await capture('release-room-local-pager-phone');
  }
  // Old snapshots without optional kind/hint still have the default menu presentation.
  await reload('home');
  await evaluate(`{const page=${state};delete page.kind;delete page.status;delete page.l;delete page.q;${win}.previewSetData(page);}`);
  assert.equal(await evaluate(`${get('release')}.getAttribute('data-kind')`), 'menu');
  assert.equal(await evaluate(`${doc}.querySelector('.r-hint')`), null);
  assert.equal(await evaluate(`${get('r-back')}.hidden`), true);
  await push({back:null});
  assert.equal(await evaluate(`${get('r-back')}.hidden`), true);
  await reload('home'); await capture('release-home-light');
  // Server chunks use the issued keys and update only after a new snapshot.
  await evaluate(`${get('r-next')}.click()`);
  await until(`${state}.page === 2 && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
  assert.equal(await evaluate(`${get('r-title')}.textContent`), 'Избранное');
  assert.ok(await evaluate(`${button('device')} && ${button('climate')}`));
  await capture('release-home-favorites');
  await evaluate(`${get('r-prev')}.click()`);
  await until(`${state}.page === 1 && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
  assert.ok(await evaluate(`${button('rooms')}`));
  // Role and favorites never move the six primary sections off the first page.
  const primaryKeys=['rooms','scenes','cameras','favorites','settings','help'];
  for (const fixture of fixtures.filter(f=>f.home?.primary)) {
    const {owner,filled}=fixture.home, ru=fixture.page.lang==='ru';
    await reload(fixture.id,{width:320});
    assert.deepEqual(fixture.page.b.slice(0,3).map(row=>row.map(b=>b[1])),[['rooms','scenes'],['cameras','favorites'],['settings','help']]);
    assert.deepEqual(await evaluate(`Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>b.dataset.key)`),primaryKeys.concat(owner?['admin']:[]));
    assert.deepEqual(await evaluate(`Array.from(${doc}.querySelectorAll('div.r-status')).map(s=>Array.from(s.children,n=>n.textContent))`),fixture.page.status,'Status is visible with the main sections');
    await click('favorites');
    await until(`${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    assert.ok(await evaluate(`document.getElementById('log').textContent.endsWith(':favorites')`),'Full favorites retains its own callback');
    await reload(fixture.id,{width:320});
    if(filled) {
      await evaluate(`${get('r-next')}.click()`);
      await until(`${state}.page === 2 && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
      assert.equal(await evaluate(`${get('r-title')}.textContent`),ru?'Избранное':'Favorites');
      assert.deepEqual(await evaluate(`Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>b.dataset.key)`),['device','climate','sensor','cameras']);
      await evaluate(`${get('r-back')}.click()`);
      await until(`${get('r-title')}.textContent === ${JSON.stringify(fixture.page.title)} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
      assert.ok(await evaluate(`document.getElementById('log').textContent.endsWith(':p0')`));
    } else {
      assert.equal(await evaluate(`${get('r-server')}.hidden`),true,'No separate page for a short status');
    }
    assert.deepEqual(await evaluate(`Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>b.dataset.key)`),primaryKeys.concat(owner?['admin']:[]));
  }
  for (const suffix of ['', '_en']) for (const width of [320,390]) {
    for (const id of ['home_two_statuses','home_statuses']) {
      const fixture=fixtures.find(f=>f.id===id+suffix);
      await reload(fixture.id,{width,theme:'dark'});
      await evaluate(`${win}.previewSetSafeArea(64)`);
      assert.ok(await evaluate(`${get('r-count')}.textContent.endsWith('.1')`),'Status never pushes primary sections to a local continuation');
      assert.deepEqual(await evaluate(`Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>b.dataset.key)`),(id==='home_statuses'?['all_statuses']:[]).concat(primaryKeys,'admin'));
      await dimensions(fixture.id+'/'+width);
      if (id==='home_statuses') {
        await click('all_statuses');
        const target=fixtures.find(f=>f.id==='all_statuses'+suffix);
        await until(`${get('r-title')}.textContent === ${JSON.stringify(target.page.title)} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
        await assertPreserved(target.page,'all statuses/'+width);
        await evaluate(`${get('r-back')}.click()`);
        await until(`${get('r-title')}.textContent === ${JSON.stringify(fixture.page.title)} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
      }
    }
  }
  await reload('home_admin_empty',{theme:'dark',width:390}); await capture('release-home-status');
  await reload('home_two_statuses',{theme:'dark',width:390}); await capture('release-home-two-statuses');
  await reload('home_statuses',{theme:'dark',width:390}); await capture('release-home-statuses');
  await reload('home_admin',{theme:'dark',width:360}); await capture('release-home-admin-dark');
  await reload('home_en',{width:320}); await capture('release-home-en-light');
  // Home distributes spare height to its menu, with the footer above native
  // message metadata. Status length and number of sections can vary independently.
  for (const id of ['home_empty','home_admin_empty','home_two_statuses','home_statuses']) {
    for (const width of [320,390,430]) for (const safe of [0,24,64]) {
      await reload(id,{width}); await evaluate(`${win}.previewSetSafeArea(${safe})`);
      const geometry=await evaluate(`(() => {
        const d=${doc}, rows=Array.from(d.querySelectorAll('.r-row'),n=>n.getBoundingClientRect());
        return {footer:d.querySelector('.r-footer').getBoundingClientRect().toJSON(),last:rows.at(-1).toJSON(),
          heights:rows.map(r=>r.height),parts:${get('r-count')}.textContent.endsWith('.1')};
      })()`);
      assert.equal(geometry.parts,true,`${id}/${width}/${safe}: main sections fit`);
      const edge=600-Math.max(16,safe);
      if(geometry.footer.height) {
        assert.equal(geometry.footer.bottom,edge,`${id}/${width}/${safe}: footer above the native clock`);
        assert.ok(geometry.footer.top-geometry.last.bottom<=20,'No empty strip between menu and footer');
      } else {
        assert.ok(geometry.last.bottom<=edge+.1 && edge-geometry.last.bottom<=8.1,'Without pagination the menu fills the space above the clock band');
      }
      assert.ok(geometry.heights.every(h=>h>=60),'Menu retains comfortable touch targets');
      await dimensions(`${id}/${width}/${safe}`);
    }
  }
  await reload('home_summary_empty',{width:320}); await capture('release-home-overview-empty');
  // Language changes use the same server-issued action path.
  await reload('settings'); await capture('release-settings');
  await click('en'); await until(`${get('r-read')}.getAttribute('aria-label') === 'Refresh'`);
  assert.equal(await evaluate(`${get('r-title')}.textContent`), 'My settings');
  await click('ru'); await until(`${get('r-read')}.getAttribute('aria-label') === 'Обновить'`);
  await reload('room', {theme:'dark'}); await capture('release-room-dark');
  const navigationPaint=await watchPaint({evaluate,doc,win});
  // The name/state opens a device card, while its adjacent command keeps a distinct key.
  assert.equal(await evaluate(`${button('device')}.classList.contains('r-nav')`), true);
  assert.equal(await evaluate(`${button('lamp_on')}.classList.contains('r-action')`), true);
  await navigationPaint(async()=>{
    await click('device'); await until(`${get('r-title')}.textContent === 'Торшер'`);
  },'Room → device');
  await capture('release-device-dark');
  assert.equal(await evaluate(`${get('r-content')}.querySelector('.r-text').textContent`), 'Состояние: Выключен.');
  await evaluate(`${get('r-back')}.click()`); await until(`${get('r-title')}.textContent === 'Гостиная'`);
  await click('ceiling'); await until(`${get('r-title')}.textContent === 'Люстра'`);
  assert.equal(await evaluate(`${get('r-content')}.querySelector('.r-text').textContent`), 'Состояние: Включена.');
  for (const [id, theme] of [['scenes','light'],['confirmation','light'],['climate','dark'],['form','light'],['admin','light']]) {
    await reload(id, {theme}); await capture('release-'+id+'-'+theme);
  }
  await reload('climate', {width:430}); await capture('release-climate-wide');
  // Compact controls follow the same request/ACK path as legacy object rows.
  await reload('climate'); await controlRequests();
  const climatePaint=await watchPaint({evaluate,doc,win});
  const geometry = () => evaluate(`Array.from(${doc}.querySelectorAll('button')).map(b=>b.getBoundingClientRect().toJSON())`);
  const beforePress = await geometry();
  await climatePaint(()=>click('temp_up'),'Start device command');
  assert.ok((await evaluate(`${win}.__requests[0].value`)).endsWith(':temp_up'));
  assert.equal(await evaluate(`${get('r-feedback')}.getBoundingClientRect().height`), 0);
  assert.equal(await evaluate(`${get('r-read')}.getAttribute('aria-label')`), 'Обновить');
  assert.deepEqual(await geometry(), beforePress, 'Press does not move or resize controls');
  await dimensions('climate/request');
  await climatePaint(()=>reply({accepted:true}),'Wait for device state');
  assert.deepEqual(await geometry(), beforePress, 'Waiting for state does not move controls');
  await climatePaint(()=>push({seq:100,rev:'a'.repeat(32)}),'Confirm device state');
  await until(`${disabled('temp_up')} === 'false'`);
  assert.equal(await evaluate(`${get('r-feedback')}.getBoundingClientRect().height`), 0);
  assert.deepEqual(await geometry(), beforePress, 'State update does not move controls');
  // Receipt is not physical state confirmation: the issued wait kind survives
  // new revisions and blocks only that device after the transport has completed.
  for (const theme of ['light', 'dark']) for (const width of [200, 360]) {
    await reload('room', {theme, width}); await controlRequests();
    // Reproduce a client that never paints CSS animations. Progress must still
    // advance via a real element, using one bounded frame loop and no requests.
    await evaluate(`(() => {
      const style=${doc}.createElement('style');
      // A CSS dependency makes headless CDP invalidate styles when emulated
      // motion returns to no-preference, so Chromium emits the change event.
      style.textContent='*,::before,::after{animation:none!important;transition:none!important}@media(prefers-reduced-motion:reduce){html{--test-motion:1}}';
      ${doc}.head.appendChild(style);
      const w=${win}, raf=w.requestAnimationFrame.bind(w);
      w.__frames={pending:0,max:0};
      w.requestAnimationFrame=callback=>{
        w.__frames.pending++;
        w.__frames.max=Math.max(w.__frames.max,w.__frames.pending);
        return raf(time=>{w.__frames.pending--;callback(time)});
      };
    })()`);
    const stable = await geometry();
    await click('lamp_on'); await click('lamp_on');
    assert.equal(await evaluate(`${win}.__requests.length`), 1);
    assert.equal(await evaluate(`${button('lamp_on')}.getAttribute('aria-busy')`), 'true');
    const ring = `${button('lamp_on')}.querySelector('i')`;
    const spinner = `${win}.getComputedStyle(${ring})`;
    assert.equal(await evaluate(`${spinner}.animationName`), 'none');
    assert.equal(await evaluate(`${spinner}.width`), '28px');
    assert.equal(await evaluate(`${spinner}.height`), '28px');
    assert.equal(await evaluate(`${win}.getComputedStyle(${button('lamp_on')}.firstChild).opacity`), '0');
    const rotation = await evaluate(`${ring}.style.transform`);
    await until(`${ring}.style.transform !== ${JSON.stringify(rotation)}`);
    assert.equal(await evaluate(`(() => {
      const r=${ring}.getBoundingClientRect(), b=${button('lamp_on')}.getBoundingClientRect();
      return Math.abs(r.x+r.width/2-b.x-b.width/2)<1 && Math.abs(r.y+r.height/2-b.y-b.height/2)<1;
    })()`), true, 'Ring stays centered while rotating');
    assert.deepEqual(await geometry(), stable, 'Immediate progress keeps button geometry');
    await reply({accepted:true});
    const waiting = rows(fixtures.find(f=>f.id==='room').page).map(row=>row.map(b=>b.key==='lamp_on'?{...b,kind:'wait'}:b));
    await push({s:'room-1',seq:100,rev:'b'.repeat(32),rows:waiting});
    assert.equal(await evaluate(disabled('lamp_on')), 'true');
    assert.equal(await evaluate(disabled('ceiling_off')), 'false', 'Other devices remain available');
    assert.deepEqual(await geometry(), stable, 'Pending HA state keeps button geometry');
    await push({s:'room-1',seq:101,rev:'c'.repeat(32)});
    await click('lamp_on');
    assert.equal(await evaluate(`${win}.__requests.length`), 1, 'New revision cannot repeat a pending action');
    await command('Emulation.setEmulatedMedia', {features:[{name:'prefers-reduced-motion',value:'reduce'}]});
    await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve(true))))');
    const reducedRotation = await evaluate(`${ring}.style.transform`);
    await evaluate('new Promise(resolve=>setTimeout(resolve,180))');
    assert.equal(await evaluate(`${ring}.style.transform`), reducedRotation, 'Reduced motion stops rotation');
    assert.deepEqual(await geometry(), stable, 'Reduced motion also preserves button geometry');
    await command('Emulation.setEmulatedMedia', {features:[{name:'prefers-reduced-motion',value:'no-preference'}]});
    await until(`${ring}.style.transform !== ${JSON.stringify(reducedRotation)}`);
    if(width===360) await capture('release-pending-'+theme);
    const unknown = waiting.map(row=>row.map(b=>b.key==='lamp_on'?{...b,kind:'uncertain'}:b));
    await push({seq:102,rev:'d'.repeat(32),rows:unknown});
    assert.equal(await evaluate(disabled('lamp_on')), 'true');
    assert.equal(await evaluate(`${button('lamp_on')}.getAttribute('aria-busy')`), 'false');
    assert.equal(await evaluate(ring), null, 'Timeout removes the spinner');
    await until(`${win}.__frames.pending === 0`);
    assert.equal(await evaluate(`${win}.__frames.max`), 1, 'New revisions never create overlapping animation loops');
    assert.equal(await evaluate(`${win}.getComputedStyle(${button('lamp_on')}.firstChild).opacity`), '1');
    assert.ok(await evaluate(`${get('r-feedback')}.textContent.includes('Нет ответа')`));
    assert.equal(await evaluate(`${get('r-read')}.getAttribute('aria-disabled')`), 'false');
    await click('lamp_on');
    assert.equal(await evaluate(`${win}.__requests.length`), 1, 'Timeout never retries control');
    await push({seq:103,rev:'e'.repeat(32),rows:waiting.map(row=>row.map(b=>b.key==='lamp_on'?{text:'Выключить',key:'lamp_off',kind:'action'}:b))});
    assert.equal(await evaluate(disabled('lamp_off')), 'false', 'Confirmed state enables the new command');
    await until(`${win}.__frames.pending === 0`);
    assert.equal(await evaluate(`${get('r-feedback')}.getBoundingClientRect().height`), 0);
    await click('lamp_off'); await reply({accepted:true});
    await push({seq:104,rev:'f'.repeat(32),rows:waiting.map(row=>row.map(b=>b.key==='lamp_on'?{text:'Выключить',key:'lamp_off',kind:'wait'}:b))});
    await until(`${button('lamp_off')}.querySelector('i').style.transform !== ''`);
    await push({seq:105,rev:'a'.repeat(32),rows:rows(fixtures.find(f=>f.id==='room').page)});
    await until(`${win}.__frames.pending === 0`);
    assert.equal(await evaluate(disabled('lamp_on')), 'false', 'Direct state confirmation stops progress and enables control');
    assert.equal(await evaluate(`${win}.__requests.length`), 2, 'Frames and state updates never send extra requests');
    await dimensions('pending/'+theme+'/'+width);
  }
  // Telegram reserves space for native message metadata when there is no caption.
  for (const width of [200, 360]) {
    await reload('long', {width});
    await evaluate(`${win}.previewSetSafeArea(64)`);
    assert.equal(await evaluate(`${win}.getComputedStyle(${get('release')}).paddingBottom`), '64px');
    assert.equal(await evaluate(`${win}.getComputedStyle(${get('release')}).paddingLeft`), '12px');
    await assertPreserved(fixtures.find(f=>f.id==='long').page, `safe-area/${width}`);
    const lastBottom = await evaluate(`Math.max(...Array.from(${doc}.querySelectorAll('button')).map(b=>b.getBoundingClientRect().bottom))`);
    assert.ok(lastBottom <= 536, 'Native metadata does not cover buttons');
    await evaluate(`${win}.previewSetSafeArea(0)`);
    await dimensions(`safe-area/cleared/${width}`);
  }
  // A newer state of the same server page keeps the user's local part.
  // Changing the screen or the server page still starts at the first part.
  await reload('long', {width:200});
  await push({s:'room-1',seq:100});
  await evaluate(`${get('r-local-next')}.click()`);
  const partBefore = await evaluate(`${get('r-local-count')}.textContent`);
  const textBefore = await evaluate(`${get('r-content')}.textContent`);
  assert.ok(partBefore.endsWith('.2'));
  await push({s:'room-1',seq:101,rev:'b'.repeat(32)});
  assert.equal(await evaluate(`${get('r-local-count')}.textContent`),partBefore);
  assert.equal(await evaluate(`${get('r-content')}.textContent`),textBefore);
  await push({s:'room-2',seq:102,rev:'c'.repeat(32)});
  assert.ok(await evaluate(`${get('r-local-count')}.textContent.endsWith('.1')`));
  await evaluate(`${get('r-local-next')}.click()`);
  await push({s:'room-2',seq:103,page:2,pages:2});
  assert.ok(await evaluate(`${get('r-local-count')}.textContent.endsWith('.1')`));
  // Both wire shapes together, malformed tuples and invalid typed values fail closed.
  for (const shape of [
    {rows:[],b:[]}, {b:null}, {b:[null]}, {b:[[{}]]}, {b:[[['short','key']]]},
    {b:[[['extra','key','nav','extra']]]}, {b:[[[4,'key','nav']]]},
    {b:[[['Label','bad:key','nav']]]}, {b:[[['Label','key','unknown']]]},
    {q:null},{q:[]},{q:['List',null]},{q:['List',null,'bad:key']},{q:[1,null,null]},{l:'yes'}
  ]) {
    await reload('home'); await controlRequests();
    const bad={...fixtures[0].page}; delete bad.rows; Object.assign(bad,shape);
    await evaluate(`${win}.previewSetData(${JSON.stringify(bad)})`);
    assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
    assert.deepEqual(await evaluate(`${win}.__requests.map(r=>r.value)`), ['r1:home'], 'Invalid state only requests a fresh Home');
  }
  // The thermostat's original Back lives on chunk 2; its persistent control works on both.
  for (const lang of ['ru','en']) for (const theme of ['light','dark']) for (const serverPage of [1,2]) {
    await reload(lang === 'ru' ? 'climate' : 'climate_en', {theme,width:200});
    if (serverPage === 2) {
      await serverStep();
      await until(`${state}.page===2 && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    }
    assert.equal(await evaluate(`${get('r-back')}.textContent`), lang === 'ru' ? '‹ Назад' : '‹ Back');
    assert.equal(await evaluate(`${get('r-back')}.getAttribute('aria-disabled')`), 'false');
    await evaluate(`${get('r-back')}.click()`);
    await until(`${get('r-title')}.textContent === ${JSON.stringify(lang === 'ru' ? 'Гостиная' : 'Living room')} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
  }
  // Returning from each local part dispatches exactly one canonical action, not pagination.
  for (const id of ['confirmation','confirmation_en','long']) for (const theme of ['light','dark']) {
    const fixture = fixtures.find(f=>f.id===id);
    await reload(id, {theme,width:200});
    const {parts} = await collectParts('persistent back/'+id, fixture.page);
    for (let part=0;part<parts;part++) {
      await reload(id, {theme,width:200});
      for (let i=0;i<part;i++) await evaluate(`${get('r-local-next')}.click()`);
      await controlRequests();
      await evaluate(`${get('r-back')}.click();${get('r-back')}.click()`);
      assert.equal(await evaluate(`${win}.__requests.length`), 1);
      assert.equal(await evaluate(`${win}.__requests[0].value`), `r1:${fixture.page.rev}:${fixture.page.back}`);
      assert.equal(await evaluate(`${get('r-title')}.textContent`), fixture.page.title);
      assert.equal(await evaluate(`${get('r-back')}.getAttribute('aria-disabled')`), 'true');
      await reply({accepted:true});
      const destination = fixtures.find(f=>f.id === fixture.page.back + (fixture.page.lang==='en' && fixture.page.back==='scenes' ? '_en' : ''));
      await evaluate(`${win}.previewSetData(${JSON.stringify({...destination.page,seq:100,rev:'a'.repeat(32)})})`);
      await until(`${get('r-title')}.textContent === ${JSON.stringify(destination.page.title)} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    }
  }
  // Malformed keys fail before request; unknown well-formed keys are rejected by the server map.
  await reload('climate'); await controlRequests(); await push({back:'bad:key'});
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  await evaluate(`${get('r-back')}.click()`);
  assert.deepEqual(await evaluate(`${win}.__requests.map(r=>r.value)`), ['r1:home']);
  await reload('climate'); await push({back:'unknown_key'});
  await evaluate(`${get('r-back')}.click()`);
  await until(`${get('r-title')}.textContent === 'Доступ закрыт'`);
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  // A failed Back follows ordinary recovery and never retries or bypasses the lock.
  await reload('climate', {scenario:'error'}); await evaluate(`${get('r-back')}.click()`);
  await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
  assert.equal(await evaluate(`${get('r-back')}.getAttribute('aria-disabled')`), 'true');
  await evaluate(`${get('r-read')}.click()`);
  await until(`${get('r-back')}.getAttribute('aria-disabled') === 'false' && ${get('r-feedback')}.textContent === ''`);
  // Existing callback menus keep their entire continuation and both directions.
  for (const lang of ['ru','en']) {
    await reload(lang === 'ru' ? 'admin' : 'admin_en');
    for (const page of [2,3]) {
      await serverStep();
      await until(`${state}.page===${page} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    }
    for (const page of [2,1]) {
      await serverStep('prev');
      await until(`${state}.page===${page} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    }
    assert.equal(await evaluate(`${get('r-read')}.getAttribute('aria-label')`), lang === 'ru' ? 'Обновить' : 'Refresh');
  }
  // Form prompts preserve every instruction; typing remains in the Telegram chat.
  await reload('form', {width:200});
  const form = fixtures.find(f => f.id === 'form').page;
  await assertPreserved(form, 'form hint/200');
  assert.equal(await evaluate(`${doc}.querySelector('input,textarea,select')`), null);
  // A failed confirmation never retries its action and returns through the original Cancel key.
  for (const lang of ['ru','en']) {
    await reload(lang === 'ru' ? 'confirmation' : 'confirmation_en', {scenario:'error'});
    await click('run_scene');
    await until(`${get('r-feedback')}.textContent.includes(${JSON.stringify(lang === 'ru' ? 'Нет ответа' : 'No response')})`);
    assert.equal(await evaluate(disabled('run_scene')), 'true');
    await evaluate(`${get('r-read')}.click()`);
    await until(`${get('r-back')}.getAttribute('aria-disabled') === 'false' && ${get('r-feedback')}.textContent === ''`);
    await evaluate(`${win}.TelegramWidget.request=async key=>{${win}.__lastRequest=key;return {result:JSON.stringify({accepted:true})}}`);
    await evaluate(`${get('r-back')}.click()`);
    assert.ok((await evaluate(`${win}.__lastRequest`)).endsWith(':scenes'));
    const next = {...fixtures.find(f=>f.id === (lang==='ru'?'scenes':'scenes_en')).page,seq:100,rev:'a'.repeat(32)};
    await evaluate(`${win}.previewSetData(${JSON.stringify(next)})`);
    await until(`${get('r-title')}.textContent === ${JSON.stringify(next.title)} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
  }
  // A command timeout blocks every action; read recovers through accepted + push.
  await reload('home', {scenario:'error'}); await click('rooms');
  await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
  assert.equal(await evaluate(disabled('rooms')), 'true');
  await evaluate(`${get('r-read')}.click()`);
  await until(`${get('r-feedback')}.textContent === '' && ${disabled('rooms')} === 'false'`);
  // An ACK without its promised update expires once and never sends an automatic retry.
  await reload('home'); await controlRequests();
  await evaluate(`{
    const w=${win}, original=w.setTimeout;
    w.setTimeout=(fn,delay,...args)=>original(fn,delay===12000?500:delay,...args);
  }`);
  await click('rooms'); await reply({accepted:true});
  await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
  assert.equal(await evaluate(`${win}.__requests.length`), 1);
  await push({seq:2,rev:'2'.repeat(32)});
  assert.equal(await evaluate(disabled('rooms')), 'true', 'Late command data does not recover after ACK timeout');
  await evaluate(`${get('r-read')}.click()`); await reply({accepted:true});
  await push({seq:3,rev:'3'.repeat(32)});
  await until(`${disabled('rooms')} === 'false'`);
  // Both message/ACK orders recover, but never before the successful read ACK.
  for (const dataBeforeAck of [false, true]) {
    await reload('home'); await failAction();
    await evaluate(`${get('r-read')}.click()`);
    assert.ok((await evaluate(`${win}.__requests.at(-1).value`)).endsWith(':read'));
    if (dataBeforeAck) {
      await push({seq:10,rev:'a'.repeat(32)});
      assert.equal(await evaluate(disabled('rooms')), 'true');
    }
    await reply({accepted:true});
    if (!dataBeforeAck) {
      assert.equal(await evaluate(disabled('rooms')), 'true');
      await push({seq:10,rev:'a'.repeat(32)});
    }
    await until(`${disabled('rooms')} === 'false' && ${get('r-feedback')}.textContent === ''`);
    await click('rooms');
    assert.equal(await evaluate(`${win}.__requests.at(-1).value`), 'r1:'+'a'.repeat(32)+':rooms');
  }
  // Same/older read replies and unsolicited late data cannot clear a failed command.
  for (const seq of [0, 1]) {
    await reload('home'); await failAction();
    await evaluate(`${get('r-read')}.click()`);
    await reply({page:{...fixtures[0].page,seq}});
    await until(`${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    assert.equal(await evaluate(disabled('rooms')), 'true');
    assert.ok(await evaluate(`${get('r-feedback')}.textContent.includes('Нет ответа')`));
  }
  await reload('home'); await failAction();
  await push({seq:9,title:'Новый экран',rev:'9'.repeat(32)});
  assert.equal(await evaluate(disabled('rooms')), 'true');
  await push({seq:2,title:'Старый экран',rev:'2'.repeat(32)});
  assert.equal(await evaluate(`${get('r-title')}.textContent`), 'Новый экран');
  await evaluate(`${get('r-read')}.click()`); await reply({accepted:true});
  await push({seq:10,rev:'a'.repeat(32)});
  await until(`${disabled('rooms')} === 'false'`);
  // A delayed tap can refer to a superseded server revision. A bounded current
  // page reply resynchronizes the widget without an access-denied screen or
  // automatically retrying the obsolete command. The next tap uses the new key.
  for (const id of ['room', 'climate', 'admin']) {
    const page=fixtures.find(f=>f.id===id).page, target=visibleActions(page)[0][0];
    await reload(id); await controlRequests();
    await click(target);
    await reply({page:{...page,seq:page.seq+1,rev:'b'.repeat(32)}});
    await until(`${disabled(target)} === 'false'`);
    assert.equal(await evaluate(`${get('r-title')}.textContent`), page.title);
    assert.equal(await evaluate(`${get('r-feedback')}.textContent`), '');
    assert.equal(await evaluate(`${win}.__requests.length`), 1, 'Stale tap is never replayed automatically');
    await click(target);
    assert.equal(await evaluate(`${win}.__requests.at(-1).value`), 'r1:'+'b'.repeat(32)+':'+target);
  }
  // Only one request may be in flight; confirmed content never changes optimistically.
  await reload('home'); await controlRequests();
  await click('rooms'); await click('rooms'); await evaluate(`${get('r-read')}.click()`);
  assert.equal(await evaluate(`${win}.__requests.length`), 1);
  assert.equal(await evaluate(`${get('r-title')}.textContent`), 'Мой дом');
  await reply({accepted:true});
  assert.equal(await evaluate(disabled('rooms')), 'true');
  // Both denial forms hide private content, including during a pending request.
  await evaluate(`${win}.previewSetData({denied:true})`);
  await until(`${get('r-title')}.textContent === 'Доступ закрыт'`);
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  assert.equal(await evaluate(`${win}.__requests.at(-1).value`),'r1:home');
  await reply({denied:true});
  await evaluate(`${win}.previewSetData({denied:true})`);
  assert.equal(await evaluate(`${win}.__requests.length`),2,'Denied recovery does not loop or replay the original action');
  await reload('home'); await controlRequests(); await click('rooms');
  await reply({denied:true});
  await until(`${win}.__requests.length===2`);
  assert.equal(await evaluate(`${win}.__requests[1].value`),'r1:home','Denied answer requests Home once');
  await evaluate(`${win}.__requests[1].reject(Error('offline'))`);
  await evaluate(`${win}.previewSetData({denied:true})`);
  assert.equal(await evaluate(`${win}.__requests.length`),2,'Failed recovery is not retried automatically');
  await reload('home', {scenario:'denied'}); await click('rooms');
  await until(`${get('r-title')}.textContent === 'Доступ закрыт'`);
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  await reload('home', {scenario:'forward'});
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  assert.ok(await evaluate(`${get('r-feedback')}.textContent.includes('/start')`));
  // Arbitrary strings stay text; long title/context stay available in local parts.
  await reload('home', {width:200});
  const longPage = {...fixtures[0].page,seq:50,title:'<img src=x onerror=alert(1)> '+'Ш'.repeat(80),context:'Г'.repeat(80),hint:'<img src=x onerror=alert(1)>'};
  await evaluate(`${win}.previewSetData(${JSON.stringify(longPage)})`);
  assert.equal(await evaluate(`${doc}.querySelector('img')`), null);
  const longContent = await assertPreserved(longPage, 'long titles/200');
  assert.ok(longContent.text.includes(longPage.title));
  assert.ok(longContent.text.includes(longPage.context));
  // Mobile shell and native widget remain within their separate width constraints.
  await command('Emulation.setDeviceMetricsOverride', {width:390,height:1300,deviceScaleFactor:1,mobile:true});
  await reload('home', {width:320});
  await evaluate(`document.getElementById('chat').scrollIntoView({block:'start'})`);
  await capture('release-mobile');
  await dimensions('mobile');
  assert.ok(await evaluate('document.documentElement.scrollWidth <= innerWidth'), 'Release mobile shell overflow');
  console.log(`Release checks passed: ${fixtures.length} fixtures × 2 themes × 4 widths, persistent Back across server/local pages, all text/actions, separate device navigation/control, RU/EN, timeout/read recovery, both ACK races, stale seq, duplicate taps, denial, forwards and mobile.`);
}
