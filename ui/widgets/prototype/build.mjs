// Feasibility artifact, deliberately separate from production build.mjs.
// Oversize output is measurable/viewable, but must never be sent to Telegram.
import {readFile, writeFile, mkdir, rm} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {deflateSync} from 'node:zlib';
import {createHash} from 'node:crypto';
import {build} from 'esbuild';
import {minify} from 'terser';
import {compactRelease} from '../private-names.mjs';
import {embed} from '../preview/embed.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const read = file => readFile(root + file, 'utf8');
const generated = ['generated-menu.js', 'generated-media.js', 'generated-assets.json'];
function once(source, from, to) {
  if (source.split(from).length !== 2) throw Error('Renderer changed; review prototype adapter: ' + from);
  return source.replace(from, to);
}
function factory(source, isMenu) {
  source = source.replace("'./graph-render.js'", "'../src/graph-render.js'");
  source = once(source, '\n(() => {', '\nexport default function(api, runtime, live' + (isMenu ? '' : ', CHART') + ') {\nconst {setTimeout,clearTimeout,requestAnimationFrame,cancelAnimationFrame,ResizeObserver}=runtime;');
  source = once(source, isMenu ? 'const api = window.TelegramWidget, el' : 'const api=window.TelegramWidget, el', 'const el');
  source = once(source, 'function render() {', 'function render() {\nif(!live())return;');
  source = once(source, '\n})();', '\n}');
  return isMenu ? compactRelease(source) : source;
}
try {
  await writeFile(root + 'prototype/generated-menu.js', factory(await read('src/r1.js'), true));
  await writeFile(root + 'prototype/generated-media.js', factory(await read('src/graph.js'), false));
  const assets = {};
  for (const name of ['release', 'graph', 'range']) {
    const source = await read('dist/' + name + '.html');
    const match = source.match(/<style>([\s\S]*?)<\/style>([\s\S]*?)<script>/);
    if (!match) throw Error('Unexpected production HTML: ' + name);
    assets[name] = {css: match[1], html: match[2]};
  }
  await writeFile(root + 'prototype/generated-assets.json', JSON.stringify(assets));
  const result = await build({entryPoints:[root+'prototype/unified.js'], bundle:true, write:false,
    format:'esm', target:'es2020', charset:'utf8', define:{GRAPH:'false'}, minifySyntax:true});
  const script = await minify(result.outputFiles[0].text, {ecma:2020, compress:{passes:2},format:{ascii_only:true}});
  const html = '<!doctype html><meta charset=utf-8><style id="u-style"></style><div id="u-host"></div><script>' + script.code + '</script>';
  const bytes = Buffer.byteLength(html), zlibBytes = deflateSync(html, {level:9}).length;
  const manifest = {screen:'unified-prototype', bytes, zlibBytes, limit:4096,
    sha256:createHash('sha256').update(html).digest('hex'),
    fitsTelegram:bytes<=32768&&zlibBytes<=4096, production:false, liveIntegrationVerified:false};
  await mkdir(root+'prototype/dist', {recursive:true});
  await writeFile(root+'prototype/dist/unified.html', html);
  await writeFile(root+'prototype/dist/manifest.json', JSON.stringify(manifest,null,2)+'\n');
  await rm(root+'prototype/dist/browser-report.json',{force:true});
  const fixtures = [...JSON.parse(await read('preview/release-fixtures.json')), ...JSON.parse(await read('preview/graph-fixtures.json'))];
  // The production fixtures were split across previews; connect the existing
  // room history button to the graph fixture without changing the card itself.
  for (const fixture of fixtures) if (['room','room_en'].includes(fixture.id)) {
    fixture.links = {...fixture.links, sensor:'numeric'+(fixture.page.lang==='en'?'_en':'')};
  }
  // Reuse the current mock and fixtures; cross-template buttons now have targets
  // in this one preview, with no second iframe or document reload in the widget.
  const preview = embed(await read('preview/release.html'), {PAYLOAD:html,FIXTURES:fixtures,MANIFEST:manifest});
  await writeFile(root+'prototype/dist/unified-preview.html', preview.replaceAll('../../../docks/', '../../../../docks/').replace('Предпросмотр сообщения', 'Прототип · один документ').replace('Пример данных · реальный дом не подключён',
    'Прототип · '+(manifest.fitsTelegram?'лимит Telegram соблюдён':'превышает лимит Telegram, только браузерная проверка')));
  console.log(JSON.stringify(manifest));
} finally {
  await Promise.all(generated.map(name => rm(root+'prototype/'+name, {force:true})));
}
