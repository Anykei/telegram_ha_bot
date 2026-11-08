// Run with an installed official Telegram SDK checkout, without HA/Telegram:
// HA_WIDGET_SDK_ROOT=/path/to/widget-sandbox node tests/sandbox-runtime.mjs
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';

assert.ok(process.env.HA_WIDGET_SDK_ROOT,'Set HA_WIDGET_SDK_ROOT to an installed Telegram widget-sandbox checkout');
const sdk=resolve(process.env.HA_WIDGET_SDK_ROOT),requireSDK=createRequire(sdk+'/package.json');
const {tsImport}=await import(pathToFileURL(requireSDK.resolve('tsx/esm/api')));
const {runWidget}=await tsImport(sdk+'/packages/core/scripts/sandbox-node.ts',import.meta.url);
const root=new URL('../',import.meta.url);
const menus=JSON.parse(await readFile(new URL('preview/release-fixtures.json',root),'utf8'));
const media=JSON.parse(await readFile(new URL('preview/graph-fixtures.json',root),'utf8'));
function page(fixture) {
  const p=structuredClone(fixture.page);
  if(p.rows){p.b=p.rows.map(row=>row.map(b=>[b.text,b.key,b.kind]));delete p.rows;}
  return p;
}
for(const [asset,fixtures] of [
  ['release',menus.filter(f=>['home','room','climate'].includes(f.id))],
  ['graph',media.filter(f=>['constant','empty','binary'].includes(f.id))],
  ['range',media.filter(f=>['range','calendar','time'].includes(f.id))]
]) {
  const pages=fixtures.map(page).filter(p=>Buffer.byteLength(JSON.stringify(p))<=1024);
  assert.ok(pages.length>=2,`${asset}: at least two transport-sized fixtures`);
  const script=[],expected=[];
  for(let i=0;i<30;i++) {
    const p={...pages[i%pages.length],seq:i+2};
    script.push({data:{chatData:JSON.stringify(p)}},{probe:"document.querySelector('h1').textContent"});
    expected.push(p.title);
  }
  const result=await runWidget(await readFile(new URL('dist/'+asset+'.html',root),'utf8'),{
    policy:'strict',cpuLimitMs:500,memoryLimitBytes:8*1024*1024,
    theme:'dark',locale:'ru',width:360,timerRounds:0,
    context:{canQuery:true,chatType:'private'},data:{chatData:JSON.stringify(pages[0])},script
  });
  assert.equal(result.stopped,undefined,`${asset}: ${result.stopped}`);
  assert.deepEqual(result.diags,[],`${asset}: sandbox diagnostics`);
  assert.deepEqual(result.logs.filter(l=>l.level==='error'),[],`${asset}: runtime errors`);
  assert.deepEqual(result.probes,expected,`${asset}: every transition finished`);
  console.log(`${asset}: 30 transitions in Telegram QuickJS, strict policy, 8 MiB heap, 500 ms task limit; max ${result.nodes} nodes / ${result.maxTaskQueries} queries per task`);
}
// The SDK harness uses stub geometry here. Browser paint-budget checks and the
// optional native layout runner cover rendering; this is not a live-client test.
