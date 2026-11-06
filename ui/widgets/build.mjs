import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { transform } from 'esbuild';

const root = fileURLToPath(new URL('.', import.meta.url));
const read = path => readFile(root + path, 'utf8');
const tokens = JSON.parse(await read('design-tokens.json'));
const declarations = entries => Object.entries(entries).map(([key, value]) => `${key}:${value};`).join('');
const block = (selector, values) => `${selector} {\n${Object.entries(values).map(([key, value]) => `  ${key}: ${value};`).join('\n')}\n}\n`;
await writeFile(root + 'src/tokens.css', block(':root', { ...tokens.shared, ...tokens.themes.light }) + block(':root[data-theme="dark"]', tokens.themes.dark));
const components = await read('src/components.css');
const used = new Set(components.match(/--ha-[a-z0-9-]+/g));
const usedTokens = values => Object.fromEntries(Object.entries(values).filter(([key]) => used.has(key)));
const tokenCSS = `:root{${declarations(usedTokens({ ...tokens.shared, ...tokens.themes.light }))}}:root[data-theme="dark"]{${declarations(usedTokens(tokens.themes.dark))}}`;
const css = await transform(tokenCSS + components, { loader: 'css', minify: true, charset: 'utf8' });
const script = await transform(await read('src/switch.js'), { minify: true, target: 'es2020', charset: 'utf8' });
const body = (await read('src/switch.html')).replace(/>\s+</g, '><').trim();
const html = `<!doctype html><html lang="ru"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Управление светом</title><style>${css.code.trim()}</style></head><body>${body}<script>${script.code.trim()}</script></body></html>`;
// Match the server's zlib framing and level using Python, as Telegram recommends.
const size = Number(execFileSync('python3', ['-c', 'import sys,zlib; print(len(zlib.compress(sys.stdin.buffer.read(),9)))'], { input: html, encoding: 'utf8' }).trim());
const bytes = Buffer.byteLength(html);
if (bytes > 32768 || size > 4096) throw Error(`Widget too large: ${bytes} raw / ${size} zlib bytes`);
const manifest = { designVersion: tokens.version, screen: 'switch', bytes, zlibBytes: size, limit: 4096, protocol: 'w1', transport: 'TelegramWidget', liveIntegrationVerified: false };
const escaped = value => JSON.stringify(value).replaceAll('<', '\\u003c');
const preview = (await read('preview/index.html'))
  .replace('__PAYLOAD__', escaped(html))
  .replace('__MOCK__', escaped(await read('preview/mock-telegram.js')))
  .replace('__MANIFEST__', escaped(manifest));
await mkdir(root + 'dist', { recursive: true });
await writeFile(root + 'dist/switch.html', html);
await writeFile(root + 'dist/preview.html', preview);
await writeFile(root + 'dist/manifest.json', JSON.stringify(manifest, null, 2) + '\n');
console.log(`switch.html: ${bytes} bytes, zlib ${size}/4096. Preview: ui/widgets/dist/preview.html`);
