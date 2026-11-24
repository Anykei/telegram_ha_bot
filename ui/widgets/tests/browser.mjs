// Local Chromium via its DevTools pipe; no server, browser download or HA access.
import { mkdir, readdir, access, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { checkTemperature } from './temperature-browser.mjs';
import { checkPanel } from './panel-browser.mjs';
import { checkRelease } from './release-browser.mjs';
import { checkGraph } from './graph-browser.mjs';
import { checkAudit } from './audit-browser.mjs';
import { checkLive } from './live-browser.mjs';
import { checkUnified } from './unified-browser.mjs';
import { openBrowser } from './browser-connection.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
let executable = process.env.HA_WIDGET_CHROME;
if (!executable && process.platform === 'darwin') {
  const cache = join(homedir(), 'Library/Caches/ms-playwright');
  for (const directory of (await readdir(cache).catch(() => [])).filter(x => /^chromium-\d+$/.test(x)).reverse()) {
    const candidate = join(cache, directory, 'chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing');
    try { await access(candidate); executable = candidate; break; } catch {}
  }
}
if (!executable) throw Error('Set HA_WIDGET_CHROME to an installed Chromium executable.');
const { command, close } = await openBrowser(executable);
async function evaluate(expression) {
  const result = await command('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw Error(JSON.stringify(result.exceptionDetails));
  return result.result.value;
}
async function until(expression) {
  for (let i = 0; i < 60; i++) {
    if (await evaluate(`(() => { try { return Boolean(${expression}); } catch { return false; } })()`)) return;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw Error('Condition failed: ' + expression);
}
const inside = `document.getElementById('widget-frame').contentDocument`;
const get = id => `${inside}.getElementById('${id}')`;
async function scenario(value) {
  await evaluate(`document.querySelector('[data-scenario="${value}"]').click()`);
  await until(`${get('name')} && ${get('state')}.textContent !== 'Нет данных'`);
}
try {
  await command('Page.enable');
  await command('Network.enable');
  // The complete page must work even with networking forcibly offline.
  await command('Network.emulateNetworkConditions', { offline: true, latency: 0, downloadThroughput: 0, uploadThroughput: 0 });
  if (process.env.HA_WIDGET_TEMPERATURE_ONLY) {
    await checkTemperature({root,command,evaluate,until});
  } else if (process.env.HA_WIDGET_LIVE_FIXTURES) {
    await checkLive({root, command, evaluate, until, file:process.env.HA_WIDGET_LIVE_FIXTURES});
  } else if (process.env.HA_WIDGET_GRAPH_ONLY) {
    await checkGraph({root,command,evaluate,until,screenshot:async()=>{}});
  } else if (process.env.HA_WIDGET_RELEASE_ONLY) {
    await checkRelease({root,command,evaluate,until,screenshot:async()=>{}});
  } else if (process.env.HA_WIDGET_UNIFIED) {
    await mkdir(root+'screenshots', {recursive:true});
    const screenshot=async name=>{
      const {data}=await command('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});
      await writeFile(root+'screenshots/unified-'+name+'.png',Buffer.from(data,'base64'));
    };
    const context={root,command,evaluate,until,screenshot,unified:true};
    if(!process.env.HA_WIDGET_PARITY_ONLY) {
      await checkRelease(context);
      await checkGraph(context);
    }
    await checkUnified(context);
  } else {
  await command('Emulation.setDeviceMetricsOverride', { width: 1280, height: 1080, deviceScaleFactor: 1, mobile: false });
  await command('Page.navigate', { url: pathToFileURL(root + 'dist/preview.html').href });
  await until(`document.getElementById('widget-frame') && ${get('power')} && ${get('state')}.textContent === 'Выключен'`);
  await mkdir(root + 'screenshots', { recursive: true });
  async function screenshot(name) {
    const { data } = await command('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
    await writeFile(root + 'screenshots/' + name + '.png', Buffer.from(data, 'base64'));
  }
  await screenshot('light');
  await evaluate(`${get('power')}.click(); ${get('power')}.click()`);
  assert.equal(await evaluate(`${get('state')}.textContent`), 'Выключен');
  await until(`${get('state')}.textContent === 'Включён'`);
  await evaluate(`document.querySelector('[data-theme="dark"]').click()`);
  await until(`${inside}.documentElement.getAttribute('data-theme') === 'dark'`);
  await screenshot('dark');
  await scenario('critical');
  await evaluate(`${get('power')}.click()`);
  await until(`!${get('confirm-panel')}.classList.contains('is-hidden')`);
  await screenshot('confirmation');
  await evaluate(`${get('confirm')}.click()`);
  await until(`${get('state')}.textContent === 'Включён'`);
  for (const width of [200, 320, 360, 430]) {
    await evaluate(`document.querySelector('[data-width="${width}"]').click()`);
    await until(`${get('power')} && ${get('state')}.textContent === 'Выключен'`);
    await evaluate(`${get('power')}.click()`);
    await until(`!${get('confirm-panel')}.classList.contains('is-hidden')`);
    assert.ok(await evaluate(`${inside}.body.scrollHeight <= 600`), `Height at ${width}px`);
    assert.ok(await evaluate(`${inside}.documentElement.scrollWidth <= ${inside}.documentElement.clientWidth`), `Overflow at ${width}px`);
    assert.deepEqual(await evaluate(`(()=>{const r=${get('refresh')}.getBoundingClientRect();return [r.width,r.height,r.top,${inside}.documentElement.clientWidth-r.right]})()`),[44,44,12,12],`Switch refresh position at ${width}px`);
  }
  await evaluate(`document.querySelector('[data-width="200"]').click()`);
  await scenario('off');
  await evaluate(`${get('name')}.textContent = 'Ш'.repeat(80); ${get('room')}.textContent = 'Ш'.repeat(80); ${get('details')}.open = true`);
  assert.ok(await evaluate(`${inside}.body.scrollHeight <= 600`), 'Long names and details height');
  assert.ok(await evaluate(`${inside}.documentElement.scrollWidth <= ${inside}.documentElement.clientWidth`), 'Long names overflow');
  await scenario('error'); await evaluate(`${get('power')}.click()`);
  await until(`${get('status')}.textContent.includes('Нет ответа')`);
  assert.equal(await evaluate(`${get('power')}.getAttribute('aria-disabled')`), 'true');
  await scenario('readonly'); await evaluate(`${get('power')}.click()`);
  assert.equal(await evaluate(`${get('state')}.textContent`), 'Выключен');
  await scenario('revoke'); await evaluate(`${get('power')}.click()`);
  await until(`${get('state')}.textContent === 'Доступ закрыт'`);
  assert.equal(await evaluate(`${get('name')}.textContent`), 'Устройство');
  await scenario('forward');
  assert.equal(await evaluate(`${get('name')}.textContent`), 'Устройство');
  await command('Emulation.setDeviceMetricsOverride', { width: 390, height: 1300, deviceScaleFactor: 1, mobile: true });
  await evaluate(`document.querySelector('[data-theme="light"]').click(); document.querySelector('[data-width="320"]').click()`);
  await scenario('off');
  await screenshot('mobile');
  assert.ok(await evaluate('document.documentElement.scrollWidth <= innerWidth'), 'Mobile overflow');
  console.log('Browser checks passed: offline preview, actions, themes, confirmation, ACL, 200–430px.');
  console.log('Screenshots: ui/widgets/screenshots/{light,dark,confirmation,mobile}.png');
  await checkTemperature({root,command,evaluate,until});
  await checkPanel({root, command, evaluate, until, screenshot});
  await checkRelease({root, command, evaluate, until, screenshot});
  await checkGraph({root, command, evaluate, until, screenshot});
  await checkAudit({root, command, evaluate, until});
  }
} finally {
  await close();
}
