import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { transform } from 'esbuild';
import { embed } from './preview/embed.mjs';
import { compileWidget } from './compile-widget.mjs';

const root = fileURLToPath(new URL('.', import.meta.url));
const read = path => readFile(root + path, 'utf8');
const tokens = JSON.parse(await read('design-tokens.json'));
const declarations = entries => Object.entries(entries).map(([key, value]) => `${key}:${value};`).join('');
const block = (selector, values) => `${selector} {\n${Object.entries(values).map(([key, value]) => `  ${key}: ${value};`).join('\n')}\n}\n`;
await writeFile(root + 'src/tokens.css', block(':root', { ...tokens.shared, ...tokens.themes.light }) + block(':root[data-theme="dark"]', tokens.themes.dark));
await mkdir(root + 'dist', { recursive: true });
async function build(screen, stylesheet, title) {
  const r1 = ['release','graph','range'].includes(screen);
  let html;
  if(r1) ({html}=await compileWidget(screen));
  else {
    const components=await read('src/'+stylesheet);
    const used=new Set(components.match(/--ha-[a-z0-9-]+/g));
    const usedTokens=values=>Object.fromEntries(Object.entries(values).filter(([key])=>used.has(key)));
    const tokenCSS=`:root{${declarations(usedTokens({...tokens.shared,...tokens.themes.light}))}}:root[data-theme="dark"]{${declarations(usedTokens(tokens.themes.dark))}}`;
    const css=await transform(tokenCSS+components,{loader:'css',minify:true,charset:'utf8'});
    const script=await transform(await read(`src/${screen}.js`),{minify:true,target:'es2020',charset:'utf8'});
    const body=(await read(`src/${screen}.html`)).replace(/>\s+</g,'><').trim();
    html=`<!doctype html><html lang="ru"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${title}</title><style>${css.code.trim()}</style></head><body>${body}<script>${script.code.trim()}</script></body></html>`;
  }
  // Match the server's zlib framing and level using Python, as Telegram recommends.
  const size = Number(execFileSync('python3', ['-c', 'import sys,zlib; print(len(zlib.compress(sys.stdin.buffer.read(),9)))'], { input: html, encoding: 'utf8' }).trim());
  const bytes = Buffer.byteLength(html);
  if (bytes > 32768 || size > 4096) throw Error(`Widget too large: ${bytes} raw / ${size} zlib bytes`);
  const manifest = { designVersion: tokens.version, screen, bytes, zlibBytes: size, limit: 4096, protocol: r1 ? 'r1' : screen === 'panel' ? 'p1' : 'w1', transport: 'TelegramWidget', liveIntegrationVerified: false };
  await writeFile(root + `dist/${screen}.html`, html);
  console.log(`${screen}.html: ${bytes} bytes, zlib ${size}/4096.`);
  return {html, manifest};
}
const {html, manifest} = await build('switch', 'components.css', 'Управление светом');
const preview = embed(await read('preview/index.html'), {
  PAYLOAD: html, MOCK: await read('preview/mock-telegram.js'), MANIFEST: manifest
});
await writeFile(root + 'dist/preview.html', preview);
await writeFile(root + 'dist/manifest.json', JSON.stringify(manifest, null, 2) + '\n');
const panel = await build('panel', 'panel.css', 'Мой дом');
const panelPreview = embed(await read('preview/panel.html'), {
  PAYLOAD: panel.html, MOCK: await read('preview/panel-mock.js'), MANIFEST: panel.manifest
});
await writeFile(root + 'dist/panel-preview.html', panelPreview);
await writeFile(root + 'dist/panel-manifest.json', JSON.stringify(panel.manifest, null, 2) + '\n');
console.log('Previews: ui/widgets/dist/{preview,panel-preview}.html');

const release = await build('release');
await writeFile(root + 'dist/release-manifest.json', JSON.stringify(release.manifest, null, 2) + '\n');
const releasePreview = embed(await read('preview/release.html'), {
  PAYLOAD: release.html, FIXTURES: JSON.parse(await read('preview/release-fixtures.json')), MANIFEST: release.manifest
});
await writeFile(root + 'dist/release-preview.html', releasePreview);
const graph = await build('graph');
await writeFile(root + 'dist/graph-manifest.json', JSON.stringify(graph.manifest, null, 2) + '\n');
const graphPreview = embed(await read('preview/release.html'), {
  PAYLOAD: graph.html, FIXTURES: JSON.parse(await read('preview/graph-fixtures.json')).filter(f=>f.page.graph), MANIFEST: graph.manifest
});
await writeFile(root + 'dist/graph-preview.html', graphPreview);

const range = await build('range');
await writeFile(root + 'dist/range-manifest.json', JSON.stringify(range.manifest, null, 2) + '\n');
await writeFile(root + 'dist/range-preview.html', embed(await read('preview/release.html'), {
  PAYLOAD: range.html, FIXTURES: JSON.parse(await read('preview/graph-fixtures.json')).filter(f=>!f.page.graph), MANIFEST: range.manifest
}));
