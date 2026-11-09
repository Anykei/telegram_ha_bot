import assert from 'node:assert/strict';
import {mkdir,writeFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {mediaExpression} from '../private-names.mjs';
export async function checkTemperature({root,command,evaluate,until}) {
  const raw=evaluate,wait=until;
  evaluate=s=>raw(mediaExpression(s));until=s=>wait(mediaExpression(s));
  const doc="document.getElementById('release-frame').contentDocument",win="document.getElementById('release-frame').contentWindow";
  const state=`JSON.parse(${win}.TelegramWidget.chatData)`,apply=`${doc}.querySelector('[data-key="a0"]')`;
  await command('Emulation.setDeviceMetricsOverride',{width:1280,height:1080,deviceScaleFactor:1,mobile:false});
  await command('Page.navigate',{url:pathToFileURL(root+'dist/temperature-preview.html').href});
  await until(`${doc}.querySelector('.s-rail')`);
  for(const theme of ['light','dark'])for(const width of [200,320,360,430]) {
    await evaluate(`new Promise(resolve=>{document.getElementById('release-frame').addEventListener('load',()=>resolve(true),{once:true});theme='${theme}';width=${width};load();})`);
    await until(`${doc}.querySelector('.s-rail')`);
    const size=await evaluate(`({height:${doc}.body.scrollHeight,width:${doc}.documentElement.scrollWidth,available:${doc}.documentElement.clientWidth,buttons:[...${doc}.querySelectorAll('button')].filter(b=>b.getBoundingClientRect().width).map(b=>({h:b.getBoundingClientRect().height,w:b.getBoundingClientRect().width,bottom:b.getBoundingClientRect().bottom}))})`);
    assert.equal(size.height,600);
    assert.ok(size.width<=size.available);
    assert.ok(size.buttons.every(b=>b.h>=44&&b.w>=44&&b.bottom<=584),JSON.stringify(size));
    assert.equal(await evaluate(`${apply}.disabled`),true);
  }
  await evaluate(`${win}.__requests=[];${win}.TelegramWidget.request=value=>new Promise(resolve=>${win}.__requests.push({value,resolve}));`);
  // Real Chromium pointer capture and dragging across the rail.
  await evaluate(`document.getElementById('release-frame').scrollIntoView({block:'start'})`);
  const r=await evaluate(`(()=>{const f=document.getElementById('release-frame').getBoundingClientRect(),r=${doc}.querySelector('.s-rail').getBoundingClientRect();return {x:f.x+r.x,y:f.y+r.y+24,width:r.width}})()`);
  await command('Input.dispatchMouseEvent',{type:'mousePressed',x:r.x+r.width*.5,y:r.y,button:'left',clickCount:1});
  await command('Input.dispatchMouseEvent',{type:'mouseMoved',x:r.x+r.width*.75,y:r.y,button:'left',buttons:1});
  await command('Input.dispatchMouseEvent',{type:'mouseReleased',x:r.x+r.width*.75,y:r.y,button:'left',clickCount:1});
  assert.equal(await evaluate(`${doc}.querySelector('.s-value').textContent`),'26.5°C');
  assert.equal(await evaluate(`${win}.__requests.length`),0,'Dragging is local');
  assert.equal(await evaluate(`${apply}.disabled`),false);
  // Vertical scroll cancellation restores the draft before pointerdown.
  await evaluate(`(()=>{const rail=${doc}.querySelector('.s-rail'),capture=rail.setPointerCapture;rail.setPointerCapture=()=>{};rail.onpointerdown({pointerId:98,button:0,clientX:rail.getBoundingClientRect().left});rail.onpointercancel({pointerId:98});rail.setPointerCapture=capture;})()`);
  assert.equal(await evaluate(`${doc}.querySelector('.s-value').textContent`),'26.5°C');
  await evaluate(`${apply}.click();${apply}.click();${doc}.querySelector('.s-editor button').click()`);
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
  await evaluate(`${doc}.querySelector('.s-editor button').click()`);
  assert.equal(await evaluate(`${doc}.querySelector('.s-value').textContent`),'26°C');
  assert.equal(await evaluate(`${apply}.disabled`),false);
  await evaluate(`(()=>{const p=${state};p.seq--;p.slider.value=22000;${win}.previewSetData(p);})()`);
  assert.equal(await evaluate(`${doc}.querySelector('.s-value').textContent`),'26°C','Old pushes do not reset draft');
  await mkdir(root+'screenshots',{recursive:true});
  const clip=await evaluate(`document.getElementById('release-frame').getBoundingClientRect().toJSON()`);
  const shot=await command('Page.captureScreenshot',{format:'png',clip:{x:clip.x,y:clip.y,width:clip.width,height:600,scale:1},captureBeyondViewport:true});
  await writeFile(root+'screenshots/temperature-dark.png',Buffer.from(shot.data,'base64'));
  console.log('temperature: 8 layouts, pointer drag/cancel, local draft, Apply once, animated wait, HA confirmation and stale pushes passed');
}
