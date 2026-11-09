import assert from 'node:assert/strict';
import {mkdir,writeFile,readFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {mediaExpression} from '../private-names.mjs';
export async function checkTemperature({root,command,evaluate,until}) {
  const raw=evaluate,wait=until;
  evaluate=s=>raw(mediaExpression(s));until=s=>wait(mediaExpression(s));
  const doc="document.getElementById('release-frame').contentDocument",win="document.getElementById('release-frame').contentWindow";
  const state=`JSON.parse(${win}.TelegramWidget.chatData)`,apply=`${doc}.querySelector('[data-key="a0"]')`;
  async function checkLayout(label, expectedActions) {
    const size=await evaluate(`({height:${doc}.body.scrollHeight,width:${doc}.documentElement.scrollWidth,available:${doc}.documentElement.clientWidth,actions:${doc}.querySelectorAll('[data-key]').length,buttons:[...${doc}.querySelectorAll('button')].filter(b=>b.getBoundingClientRect().width).map(b=>({h:b.getBoundingClientRect().height,w:b.getBoundingClientRect().width,bottom:b.getBoundingClientRect().bottom,text:b.textContent,font:${win}.getComputedStyle(b).font}))})`);
    const diagnostic=label+': '+JSON.stringify(size);
    assert.equal(size.height,600,diagnostic);
    assert.ok(size.width<=size.available,diagnostic);
    assert.ok(size.buttons.every(b=>b.h>=44&&b.w>=44&&b.bottom<=584),diagnostic);
    assert.equal(size.actions,expectedActions,diagnostic);
  }
  await command('Emulation.setDeviceMetricsOverride',{width:1280,height:1080,deviceScaleFactor:1,mobile:false});
  await command('Page.navigate',{url:pathToFileURL(root+'dist/temperature-preview.html').href});
  await until(`${doc}.querySelector('.s-rail')`);
  const fixtures=JSON.parse(await readFile(root+'preview/temperature-fixtures.json','utf8'));
  // System UI resolves to different fonts on macOS, Debian and Ubuntu runners.
  // Exercise wider font metrics explicitly instead of relying on one host default.
  for(const font of ['', 'Arial, sans-serif', '"DejaVu Sans", sans-serif'])
  for(const fixture of fixtures)for(const theme of ['light','dark'])for(const width of [200,240,241,280,281,320,360,430]) {
    await evaluate(`new Promise(resolve=>{document.getElementById('release-frame').addEventListener('load',()=>resolve(true),{once:true});theme='${theme}';width=${width};document.getElementById('fixture').value='${fixture.id}';load();})`);
    await until(`${doc}.querySelector('.s-rail')`);
    if(font)await evaluate(`(()=>{const style=${doc}.createElement('style');style.textContent=${JSON.stringify('body,button {font-family:'+font+'!important}')};${doc}.head.append(style);})()`);
    const label=`${fixture.id}/${theme}/${width}/${font||'system'}`,expectedActions=fixture.page.b.flat().filter(b=>b[1]!==fixture.page.back).length;
    await checkLayout(label,expectedActions);
    assert.equal(await evaluate(`${apply}.disabled`),true);
    for(const status of ['wait','uncertain']) {
      await evaluate(`(()=>{const p=${state};p.seq++;p.b[0].forEach(b=>b[2]='${status}');p.alert='${status}'==='uncertain'?(p.lang==='ru'?'HA не подтвердил изменение. Обновите состояние.':'HA has not confirmed the change. Refresh the state.'):'';${win}.previewSetData(p);})()`);
      await checkLayout(label+'/'+status,expectedActions);
      assert.equal(await evaluate(`${apply}.disabled`),true);
    }
  }
  await evaluate(`new Promise(resolve=>{document.getElementById('release-frame').addEventListener('load',()=>resolve(true),{once:true});theme='dark';width=320;document.getElementById('fixture').value='climate_actual_normal';load();})`);
  await until(`${doc}.querySelector('.s-rail')`);
  await evaluate(`${win}.__requests=[];${win}.TelegramWidget.request=value=>new Promise(resolve=>${win}.__requests.push({value,resolve}));`);
  // Real Chromium pointer capture and dragging across the rail.
  await evaluate(`document.getElementById('release-frame').scrollIntoView({block:'start'})`);
  const r=await evaluate(`(()=>{const f=document.getElementById('release-frame').getBoundingClientRect(),r=${doc}.querySelector('.s-rail').getBoundingClientRect();return {x:f.x+r.x,y:f.y+r.y+24,width:r.width}})()`);
  await command('Input.dispatchMouseEvent',{type:'mousePressed',x:r.x+r.width*.5,y:r.y,button:'left',clickCount:1});
  await command('Input.dispatchMouseEvent',{type:'mouseMoved',x:r.x+r.width*.75,y:r.y,button:'left',buttons:1});
  await command('Input.dispatchMouseEvent',{type:'mouseReleased',x:r.x+r.width*.75,y:r.y,button:'left',clickCount:1});
  assert.equal(await evaluate(`${doc}.querySelector('.s-value').textContent`),'Цель: 26.5°C');
  assert.equal(await evaluate(`${win}.__requests.length`),0,'Dragging is local');
  assert.equal(await evaluate(`${apply}.disabled`),false);
  // Vertical scroll cancellation restores the draft before pointerdown.
  await evaluate(`(()=>{const rail=${doc}.querySelector('.s-rail'),capture=rail.setPointerCapture;rail.setPointerCapture=()=>{};rail.onpointerdown({pointerId:98,button:0,clientX:rail.getBoundingClientRect().left});rail.onpointercancel({pointerId:98});rail.setPointerCapture=capture;})()`);
  assert.equal(await evaluate(`${doc}.querySelector('.s-value').textContent`),'Цель: 26.5°C');
  await evaluate(`${apply}.click();${apply}.click()`);
  assert.equal(await evaluate(`${win}.__requests.length`),1,'Only Apply sends a command');
  assert.ok((await evaluate(`${win}.__requests[0].value`)).endsWith(':s26500'));
  assert.equal(await evaluate(`${apply}.disabled`),true);
  assert.equal(await evaluate(`${doc}.querySelector('.s-rail').getAttribute('aria-disabled')`),'true');
  await evaluate(`${win}.__requests[0].resolve({result:JSON.stringify({accepted:true})})`);
  await until(`${apply}.querySelector('i')?.style.transform`);
  const angle=await evaluate(`${apply}.querySelector('i').style.transform`);
  await new Promise(r=>setTimeout(r,120));
  assert.notEqual(await evaluate(`${apply}.querySelector('i').style.transform`),angle,'Spinner rotates while waiting');
  await evaluate(`(()=>{const p=${state};p.seq++;p.b[0][0][2]='wait';${win}.previewSetData(p);})()`);
  assert.equal(await evaluate(`${apply}.disabled`),true,'ACK/new page does not mean HA confirmed');
  assert.equal(await evaluate(`${doc}.querySelector('.s-rail').getAttribute('aria-disabled')`),'true');
  await evaluate(`(()=>{const p=${state};p.seq++;p.slider.value=26500;p.b[0][0][2]='action';${win}.previewSetData(p);})()`);
  assert.equal(await evaluate(`${doc}.querySelector('.s-rail').getAttribute('aria-disabled')`),'false');
  assert.equal(await evaluate(`${apply}.disabled`),true,'Confirmed value cannot be applied twice');
  await evaluate(`(()=>{const rail=${doc}.querySelector('.s-rail'),capture=rail.setPointerCapture;rail.setPointerCapture=()=>{};rail.onpointerdown({pointerId:99,button:0,clientX:rail.getBoundingClientRect().left});rail.onpointerup({pointerId:99,clientX:rail.getBoundingClientRect().left});rail.setPointerCapture=capture;})()`);
  assert.equal(await evaluate(`${doc}.querySelector('.s-value').textContent`),'Цель: 16°C');
  assert.equal(await evaluate(`${apply}.disabled`),false);
  await evaluate(`(()=>{const p=${state};p.seq--;p.slider.value=22000;${win}.previewSetData(p);})()`);
  assert.equal(await evaluate(`${doc}.querySelector('.s-value').textContent`),'Цель: 16°C','Old pushes do not reset draft');
  await evaluate(`(()=>{const p=${state};p.seq+=3;p.b[0][0][2]='uncertain';p.alert='HA не подтвердил температуру. Обновите состояние.';${win}.previewSetData(p);})()`);
  assert.equal(await evaluate(`${apply}.disabled`),true);
  assert.ok(await evaluate(`${doc}.body.textContent.includes('HA не подтвердил')`));
  assert.equal(await evaluate(`${doc}.getElementById('r-read').disabled`),false,'Read remains available after unconfirmed command');
  await evaluate(`${doc}.getElementById('r-read').click()`);
  assert.ok((await evaluate(`${win}.__requests.at(-1).value`)).endsWith(':read'));
  await evaluate(`(()=>{const p=${state};p.seq++;p.b[0][0][2]='action';p.alert='';${win}.__requests.at(-1).resolve({result:JSON.stringify({page:p})});})()`);
  await until(`${doc}.querySelector('.s-rail').getAttribute('aria-disabled')==='false'`);
  assert.equal(await evaluate(`${win}.__requests.length`),2,'Recovery only reads; it never repeats Apply');
  await evaluate(`(()=>{const p=${state};p.seq+=4;p.s='another-thermostat';p.alert='';p.b[0][0][2]='action';${win}.previewSetData(p);})()`);
  assert.equal(await evaluate(`${apply}.disabled`),true,'A different thermostat resets the draft even with identical limits');
  assert.ok(await evaluate(`${doc}.querySelector('.s-mark').style.left`),'Confirmed target remains marked on the rail');
  const power=`${doc}.querySelector('[data-key="a1"]')`;
  await evaluate(`(()=>{const rail=${doc}.querySelector('.s-rail'),capture=rail.setPointerCapture;rail.setPointerCapture=()=>{};const x=rail.getBoundingClientRect().left+rail.getBoundingClientRect().width/2;rail.onpointerdown({pointerId:100,button:0,clientX:x});rail.onpointerup({pointerId:100,clientX:x});rail.setPointerCapture=capture;})()`);
  const draft=await evaluate(`${doc}.querySelector('.s-value').textContent`);
  await evaluate(`${power}.click();${power}.click()`);
  assert.equal(await evaluate(`${win}.__requests.length`),3,'Power only sends once');
  assert.ok((await evaluate(`${win}.__requests.at(-1).value`)).endsWith(':a1'),'Power never submits a slider draft');
  assert.equal(await evaluate(`${power}.disabled`),true);
  assert.equal(await evaluate(`${doc}.querySelector('.s-rail').getAttribute('aria-disabled')`),'true');
  await evaluate(`${win}.__requests.at(-1).resolve({result:JSON.stringify({accepted:true})})`);
  await until(`${power}.querySelector('i')?.style.transform`);
  const powerAngle=await evaluate(`${power}.querySelector('i').style.transform`);
  await new Promise(r=>setTimeout(r,120));
  assert.notEqual(await evaluate(`${power}.querySelector('i').style.transform`),powerAngle,'Power spinner rotates while awaiting HA');
  await evaluate(`(()=>{const p=${state};p.seq++;p.b[0].forEach(b=>b[2]='wait');${win}.previewSetData(p);})()`);
  assert.equal(await evaluate(`${power}.disabled`),true,'Service ACK cannot confirm the mode');
  await evaluate(`(()=>{const p=${state};p.seq++;p.b[0].forEach(b=>b[2]='action');p.b[0][1][0]='Включить отопление';p.body='Сейчас: 21°C\\nРежим: выключен';${win}.previewSetData(p);})()`);
  assert.equal(await evaluate(`${power}.disabled`),false,'Confirmed mode unlocks power');
  assert.equal(await evaluate(`${doc}.querySelector('.s-value').textContent`),draft,'Mode change preserves the local draft');
  await mkdir(root+'screenshots' ,{recursive:true});
  const clip=await evaluate(`document.getElementById('release-frame').getBoundingClientRect().toJSON()`);
  const shot=await command('Page.captureScreenshot',{format:'png',clip:{x:clip.x,y:clip.y,width:clip.width,height:600,scale:1},captureBeyondViewport:true});
  await writeFile(root+'screenshots/temperature-dark.png',Buffer.from(shot.data,'base64'));
  console.log('temperature: system/Arial/DejaVu Sans at 200–430px, normal/wait/uncertain layouts, slider and heating controls, local draft, single dispatch, animated wait, HA confirmation and stale pushes passed');
}
