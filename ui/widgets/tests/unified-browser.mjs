import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {releaseExpression} from '../private-names.mjs';

export async function checkUnified({root,command,evaluate,until}) {
  const fixtures=[...JSON.parse(await readFile(root+'preview/release-fixtures.json','utf8')),
    ...JSON.parse(await readFile(root+'preview/graph-fixtures.json','utf8'))];
  const doc="document.getElementById('release-frame').contentDocument";
  const win="document.getElementById('release-frame').contentWindow";
  const asset=p=>p.graph?'graph':['range','calendar','time'].includes(p.kind)?'range':'release';
  const find=kind=>fixtures.find(f=>f.page.kind===kind);
  const route=[fixtures.find(f=>f.id==='home'),find('chart'),find('range'),find('calendar'),find('time'),find('chart'),fixtures.find(f=>f.id==='home')];
  assert.ok(route.every(Boolean));
  const navigate=async name=>{
    await command('Page.navigate',{url:pathToFileURL(root+name).href});
    await until(`${doc}?.querySelector('[data-kind]')`);
  };
  async function select(id,theme,width) {
    await evaluate(`new Promise(resolve=>{const f=document.getElementById('release-frame');f.addEventListener('load',()=>resolve(true),{once:true});theme=${JSON.stringify(theme)};width=${width};document.getElementById('fixture').value=${JSON.stringify(id)};document.getElementById('fixture').dispatchEvent(new Event('change'));})`);
    await until(`document.getElementById('release-frame').clientHeight===600`);
  }
  const snapshot=async()=>evaluate(`(()=>{
    const d=${doc},root=d.querySelector('[data-kind]');
    return Array.from(root.querySelectorAll('*')).filter(n=>!['path','svg'].includes(n.localName)).map(n=>{
      const r=n.getBoundingClientRect(),s=d.defaultView.getComputedStyle(n);
      return {tag:n.localName,text:n.childElementCount?'':n.textContent,x:r.x,y:r.y,w:r.width,h:r.height,color:s.color,bg:s.backgroundColor,font:s.font,line:s.lineHeight,radius:s.borderRadius};
    });
  })()`);
  let visualComparisons=0;
  await navigate('prototype/dist/unified-preview.html');
  await select('home','dark',360);
  await evaluate(`window.__navigationDocument=${doc}`);
  for (const [key,id] of [['rooms','rooms'],['room','room'],['sensor','numeric'],['period','range'],
    ['a0','calendar'],['back','range'],['a1','time'],['back','range'],['a4','numeric'],
    ['back','room'],['back','rooms'],['back','home']]) {
    if(key==='back') {
      await evaluate(`(()=>{const d=${doc};(d.getElementById('r-back')||d.getElementById('back')).click();})()`);
    } else await evaluate(`${doc}.querySelector('[data-key="${key}"]').click()`);
    const expected=fixtures.find(f=>f.id===id).page;
    await until(`JSON.parse(${win}.TelegramWidget.chatData).title===${JSON.stringify(expected.title)}`);
    assert.equal(await evaluate(`${doc}===window.__navigationDocument`),true,`Click to ${id}: same document`);
    await until(`!${doc}.querySelector('button[aria-label="Обновить"]')?.disabled && ${doc}.querySelector('button[aria-label="Обновить"]')?.getAttribute('aria-disabled')!=='true'`);
  }
  // Compare every real fixture's initial geometry and styling with its shipping
  // renderer. Existing suites above traverse every local part and every action.
  for(const theme of ['light','dark']) for(const width of [200,320,360,430]) {
    const expected=new Map();
    for(const name of ['release','graph','range']) {
      await navigate('dist/'+name+'-preview.html');
      for(const f of fixtures.filter(f=>asset(f.page)===name)) {
        await select(f.id,theme,width);
        expected.set(f.id,await snapshot());
      }
    }
    await navigate('prototype/dist/unified-preview.html');
    for(const f of fixtures) {
      await select(f.id,theme,width);
      assert.deepEqual(await snapshot(),expected.get(f.id),`${f.id}/${theme}/${width}: unchanged visual geometry and styles`);
      visualComparisons++;
    }
    console.log(`Unified visual comparison: ${theme}/${width}, ${fixtures.length} screens match`);
  }
  for(const theme of ['light','dark']) {
    await navigate('prototype/dist/unified-preview.html');
    await select('home',theme,360);
    await evaluate(`window.__widgetDocument=${doc};window.__widgetWindow=${win};${win}.__requests=[];${win}.TelegramWidget.request=raw=>new Promise(resolve=>${win}.__requests.push({raw,resolve}));`);
    let seq=100;
    for(const f of [...route,...route]) {
      const page={...f.page,seq:seq++,rev:seq.toString(16).padStart(32,'0')};
      await evaluate(`${win}.previewSetData(${JSON.stringify(page)})`);
      assert.equal(await evaluate(`${doc}===window.__widgetDocument&&${win}===window.__widgetWindow`),true,'No new document/message context');
      const title=asset(page)==='release'?releaseExpression(`${doc}.getElementById('r-title').textContent`):`${doc}.getElementById('r-title').textContent`;
      assert.equal(await evaluate(title),page.title);
    }
    // An ACK itself can change the renderer, before a push notification arrives.
    const refresh=releaseExpression(`${doc}.getElementById('r-read')`);
    await evaluate(`${refresh}.click()`);
    const chart={...find('chart').page,seq:seq++,rev:seq.toString(16).padStart(32,'0')};
    await evaluate(`${win}.__requests.at(-1).resolve({result:JSON.stringify({page:${JSON.stringify(chart)}})})`);
    await until(`${doc}.getElementById('g-box')`);
    assert.equal(await evaluate(`${doc}===window.__widgetDocument`),true,'ACK navigation keeps the document');
    // Late completion of a disposed screen cannot overwrite the newer screen.
    await evaluate(`${doc}.getElementById('r-read').click()`);
    const home={...route[0].page,seq:seq++,rev:seq.toString(16).padStart(32,'0')};
    await evaluate(`${win}.previewSetData(${JSON.stringify(home)});${win}.__requests.at(-1).resolve({result:JSON.stringify({page:${JSON.stringify({...chart,seq:1})}})})`);
    await until(releaseExpression(`${doc}.getElementById('r-title')?.textContent===${JSON.stringify(home.title)}`));
    assert.equal(await evaluate(`${doc}===window.__widgetDocument`),true,'Late ACK keeps current document');
    // Revocation also blocks late ACKs that would otherwise switch renderers.
    await evaluate(`${refresh}.click();${win}.previewSetData({denied:true})`);
    const late={...find('chart').page,seq:seq++,rev:seq.toString(16).padStart(32,'0')};
    await evaluate(`${win}.__requests.at(-2).resolve({result:JSON.stringify({page:${JSON.stringify(late)}})})`);
    await until(releaseExpression(`${doc}.getElementById('r-title')?.textContent==='Доступ закрыт'`));
    assert.equal(await evaluate(`${doc}.querySelector('[data-key]')===null`),true,'Late cross-renderer ACK cannot restore private controls');
    // Only the authorized Home recovery response can reopen the revoked view.
    const recovered={...home,seq:seq++,rev:seq.toString(16).padStart(32,'0')};
    await evaluate(`${win}.__requests.at(-1).resolve({result:JSON.stringify({page:${JSON.stringify(recovered)}})})`);
    await until(releaseExpression(`${doc}.getElementById('r-title')?.textContent===${JSON.stringify(home.title)}`));
    assert.equal(await evaluate(`${doc}===window.__widgetDocument`),true,'Home recovery uses the existing document');
  }
  const manifest=JSON.parse(await readFile(root+'prototype/dist/manifest.json','utf8'));
  const report={sha256:manifest.sha256,visualComparisons,fixtures:fixtures.length,themes:2,widths:[200,320,360,430],sameDocumentTransitions:true,buttonNavigationTransitions:12,ackTransitions:true,staleAckIsolation:true,revocationAndRecovery:true};
  await writeFile(root+'prototype/dist/browser-report.json',JSON.stringify(report,null,2)+'\n');
  console.log('Unified prototype passed: '+JSON.stringify(report));
}
