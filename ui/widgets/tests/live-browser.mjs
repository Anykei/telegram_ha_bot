// Render snapshots exported by the opt-in Rust HA test, with the browser fully offline.
import assert from 'node:assert/strict';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';

export async function checkLive({root, command, evaluate, until, file}) {
  const fixtures = JSON.parse(await readFile(file, 'utf8'));
  assert.ok(fixtures.length > 0, 'Live test exported pages');
  const assets = Object.fromEntries(await Promise.all(['release','graph'].map(async name => [name, await readFile(root+`dist/${name}.html`, 'utf8')])));
  const folder = join(dirname(file), 'screenshots');
  await mkdir(folder, {recursive:true, mode:0o700});
  const doc = `document.getElementById('live').contentDocument`;
  const get = id => `${doc}.getElementById('${id}')`;
  const captured = new Set();
  let combinations = 0, parts = 0;
  const failures = [];
  const {frameTree} = await command('Page.getFrameTree');
  await command('Page.setDocumentContent', {frameId:frameTree.frame.id, html:'<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><style>body{margin:0}iframe{display:block;width:100%;height:600px;border:0}</style></head><body><iframe id="live"></iframe></body></html>'});
  for (const theme of ['light','dark']) for (const width of [200,320,360,430]) {
    await command('Emulation.setDeviceMetricsOverride', {width,height:640,deviceScaleFactor:1,mobile:true});
    for (const fixture of fixtures) {
      const label = `${fixture.id}/${theme}/${width}`, page = fixture.page;
      try {
        // All source actions remain visible; no action can reach HA or Telegram from this harness.
        const boot = `globalThis.TelegramWidget={chatData:${JSON.stringify(JSON.stringify(page)).replaceAll('<','\\u003c')},colorScheme:${JSON.stringify(theme)},onEvent(){},ready(){},request:async()=>({result:'{"denied":true}'})};`;
        const html = assets[fixture.renderer].replace('<script>', '<script>'+boot+'</script><script>');
        await evaluate(`new Promise(resolve=>{const f=document.getElementById('live');f.onload=()=>resolve(true);f.srcdoc=${JSON.stringify(html)}})`);
        await until(`${get('r-title')} && ${get('r-title')}.textContent===${JSON.stringify(page.title)}`);
        let text = '', actions = [], local = 0;
        while (true) {
          assert.ok(++local < 100, `${label}: finite local pagination`);
          const actual = await evaluate(`({
            height:${doc}.body.scrollHeight,width:${doc}.documentElement.scrollWidth,available:${doc}.documentElement.clientWidth,
            text:Array.from(${doc}.querySelectorAll('.r-text')).map(n=>n.textContent).join(''),
            textFits:Array.from(${doc}.querySelectorAll('.r-text')).every(n=>n.scrollWidth<=n.clientWidth),
            actions:Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>[b.dataset.key,b.firstChild.textContent]),
            targets:Array.from(${doc}.querySelectorAll('button')).map(b=>b.getBoundingClientRect()).filter(r=>r.width).map(r=>({width:r.width,height:r.height})),
            back:${get('r-back')}.getBoundingClientRect().height,
            last:${get('r-local')}.classList.contains('r-hidden') || ${get('r-local-next')}.getAttribute('aria-disabled')==='true'
          })`);
          assert.ok(actual.height<=600, `${label}: card height ${actual.height}`);
          assert.ok(actual.width<=actual.available && actual.textFits, `${label}: no clipped text or horizontal overflow`);
          assert.ok(actual.targets.every(r=>r.width>=44 && r.height>=44), `${label}: touch targets`);
          if (page.back) assert.ok(actual.back>=44, `${label}: persistent return`);
          text += actual.text; actions.push(...actual.actions); parts++;
          const sample = `${fixture.renderer}-${page.kind}-${theme}`;
          if (local===1 && width===320 && page.page===1 && !captured.has(sample)) {
            const {data} = await command('Page.captureScreenshot', {format:'png',clip:{x:0,y:0,width,height:Math.min(600,actual.height),scale:1}});
            await writeFile(join(folder,sample+'.png'),Buffer.from(data,'base64'));
            captured.add(sample);
          }
          if (actual.last) break;
          await evaluate(`${get('r-local-next')}.click()`);
        }
        assert.ok(text.endsWith(page.alert+page.notice+page.body+(page.hint||'')), `${label}: all text preserved`);
        assert.deepEqual(actions,page.b.flat().filter(b=>b[1]!==page.back).map(b=>[b[1],b[0]]),`${label}: all actions preserved`);
        if (fixture.id.startsWith('home-') && page.page===1 && width>=320) assert.equal(local,1,`${label}: home sections on first part`);
        combinations++;
      } catch (error) {
        failures.push({page:fixture.id,theme,width,error:error.message});
      }
    }
    console.log(`Live browser ${theme}/${width}: ${combinations} passed, ${failures.length} failed`);
  }
  const report = {pages:fixtures.length,combinations,localParts:parts,screenshots:captured.size,failures};
  await writeFile(join(dirname(file),'browser-report.json'),JSON.stringify(report,null,2));
  assert.equal(failures.length,0,`Live UI browser failures; see ${join(dirname(file),'browser-report.json')}`);
  console.log(`Live browser passed: ${combinations} page/theme/width combinations, ${captured.size} screenshots.`);
}
