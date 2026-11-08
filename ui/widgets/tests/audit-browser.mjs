import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { releaseExpression } from '../private-names.mjs';

// Optional real-router catalog, generated only from the isolated Rust test DB.
// This checks rendering completeness, not the semantic title/parent contract.
export async function checkAudit({root, command, evaluate, until}) {
  if (!process.env.HA_WIDGET_AUDIT_PATH) return;
  const rawEvaluate=evaluate, rawUntil=until;
  evaluate=source=>rawEvaluate(releaseExpression(source));
  until=source=>rawUntil(releaseExpression(source));
  const inventory = JSON.parse(await readFile(process.env.HA_WIDGET_AUDIT_PATH, 'utf8'));
  assert.ok(inventory.length > 0, 'non-empty router catalog');
  const doc = "document.getElementById('release-frame').contentDocument";
  const win = "document.getElementById('release-frame').contentWindow";
  const get = id => `${doc}.getElementById('${id}')`;
  await command('Page.navigate', {url:pathToFileURL(root + 'dist/release-preview.html').href});
  await until(`${get('r-title')}`);
  let count = 0;
  for (const scheme of ['light', 'dark']) for (const size of [200, 320, 360, 430]) {
    await evaluate(`new Promise(resolve => {
      document.getElementById('release-frame').addEventListener('load',()=>resolve(true),{once:true});
      theme=${JSON.stringify(scheme)};width=${size};load();
    })`);
    await until(`${get('r-title')}`);
    for (const entry of inventory) for (const original of entry.pages) {
      const page = structuredClone(original);
      page.seq = ++count + 100;
      await evaluate(`${win}.previewSetData(${JSON.stringify(page)})`);
      const label = `${entry.route}/${entry.lang}/${scheme}/${size}/page${page.page}`;
      let text = '', actions = [], parts = 0;
      const expected = page.b.flat().filter(b=>b[1]!==page.back).map(b=>b[1]);
      const overflow=await evaluate(`({context:${get('r-context')}.scrollWidth>${get('r-context')}.clientWidth,title:${get('r-title')}.scrollWidth>${get('r-title')}.clientWidth})`);
      const fullText=(overflow.context?page.context:'')+(overflow.title?page.title:'')+page.alert+page.notice+page.body+(page.hint||'');
      while (true) {
        assert.ok(++parts <= 30, `${label}: finite pagination`);
        const state = await evaluate(`({
          text:Array.from(${doc}.querySelectorAll('.r-text')).map(n=>n.textContent).join(''),
          actions:Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>b.dataset.key),
          height:${doc}.body.scrollHeight,
          overflow:${doc}.documentElement.scrollWidth>${doc}.documentElement.clientWidth,
          feedback:${get('r-feedback')}.textContent
        })`);
        assert.equal(state.height, 600, `${label}: stable height`);
        assert.equal(state.overflow, false, `${label}: no overflow`);
        assert.ok(!state.feedback.includes('/classic'), `${label}: renderer accepts actual route`);
        text += state.text; actions.push(...state.actions);
        if (text===fullText && actions.length===expected.length) break;
        await evaluate(`${get('r-next')}.click()`);
      }
      assert.ok(text.endsWith(page.alert + page.notice + page.body + (page.hint || '')), `${label}: complete text`);
      assert.deepEqual(actions, expected, `${label}: complete actions`);
    }
  }
  console.log(`Router catalog checks passed: ${inventory.length} views, ${count} page/theme/width cases.`);
}
