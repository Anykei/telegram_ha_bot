import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';

export async function checkPanel({root, command, evaluate, until, screenshot}) {
  await command('Emulation.setDeviceMetricsOverride', {width:1280,height:1080,deviceScaleFactor:1,mobile:false});
  await command('Page.navigate', {url:pathToFileURL(root+'dist/panel-preview.html').href});
  const doc = `document.getElementById('panel-frame').contentDocument`;
  const win = `document.getElementById('panel-frame').contentWindow`;
  const get = id => `${doc}.getElementById('${id}')`;
  const action = name => `${doc}.querySelector('[data-action="${name}"]')`;
  const click = async name => { await evaluate(`${action(name)}.click()`); };
  async function screen(name) {
    await evaluate(`document.querySelector('[data-screen="${name}"]').click()`);
    await until(`${get('panel')} && ${get('panel')}.getAttribute('data-screen') === '${name}' && !${get('title')}.textContent.includes('Ожидание')`);
  }
  await until(`${get('panel')} && ${get('title')}.textContent === 'Дом'`);
  await screenshot('panel-home-light');
  await click('rooms'); await until(`${get('title')}.textContent === 'Комнаты'`);
  await click('room'); await until(`${get('title')}.textContent === 'Гостиная'`);
  await click('device'); await until(`${get('title')}.textContent === 'Торшер'`);
  await evaluate(`${action('on')}.click();${action('on')}.click()`);
  assert.equal(await evaluate(`${get('hero-value')}.textContent`), 'Выключен');
  await until(`${get('hero-value')}.textContent === 'Включён'`);
  assert.equal(await evaluate(`${win}.__panelTest.requests.filter(x=>x.endsWith(':on')).length`), 1);
  assert.equal(await evaluate(`${action('off')}.getAttribute('aria-pressed')`), 'true');
  await click('settings'); await until(`${get('title')}.textContent === 'Вид интерфейса'`);
  await screenshot('panel-settings-light');
  await click('classic'); await until(`document.getElementById('classic').hidden === false`);
  await evaluate(`Array.from(document.getElementById('classic').querySelectorAll('button')).find(b=>b.textContent === 'Торшер').click()`);
  await until(`document.getElementById('classic').textContent.includes('Включён')`);
  await evaluate(`Array.from(document.getElementById('classic').querySelectorAll('button')).find(b=>b.textContent === 'Выключить свет').click()`);
  await until(`document.getElementById('classic').textContent.includes('Выключен')`);
  await evaluate(`Array.from(document.getElementById('classic').querySelectorAll('button')).find(b=>b.textContent === 'Вид').click()`);
  await until(`document.getElementById('classic').textContent.includes('Вид интерфейса')`);
  await evaluate(`Array.from(document.getElementById('classic').querySelectorAll('button')).find(b=>b.textContent === 'HTML').click()`);
  await until(`document.getElementById('classic').hidden === true`);
  await click('device'); await until(`${get('hero-value')}.textContent === 'Выключен'`);
  // All pages fit the native widget's width and height limits in both themes.
  for (const theme of ['light','dark']) {
    await evaluate(`document.querySelector('[data-theme="${theme}"]').click()`);
    for (const name of ['home','rooms','room','device','settings']) {
      await screen(name);
      for (const width of [200,320,360,430]) {
        await evaluate(`document.querySelector('[data-width="${width}"]').click()`);
        assert.ok(await evaluate(`${doc}.body.scrollHeight <= 600`), `${name}/${theme}/${width} height`);
        assert.ok(await evaluate(`${doc}.documentElement.scrollWidth <= ${doc}.documentElement.clientWidth`), `${name}/${theme}/${width} overflow`);
        const targets = await evaluate(`Array.from(${doc}.querySelectorAll('button')).filter(b=>b.getBoundingClientRect().width).map(b=>b.getBoundingClientRect().height)`);
        assert.ok(targets.every(h=>h>=44), 'Touch targets >=44px');
      }
    }
  }
  await evaluate(`document.querySelector('[data-width="360"]').click()`);
  await screen('device'); await screenshot('panel-device-dark');
  await evaluate(`${win}.__panelTest.fail=true`); await click('on');
  await until(`${get('feedback')}.textContent.includes('Нет ответа')`);
  assert.equal(await evaluate(`${action('on')}.getAttribute('aria-disabled')`), 'true');
  await click('on'); assert.equal(await evaluate(`${win}.__panelTest.requests.length`), 1);
  await evaluate(`${win}.__panelTest.fail=false;${get('panel-refresh')}.click()`);
  await until(`${action('on')}.getAttribute('aria-disabled') === 'false'`);
  await click('on'); await until(`${get('hero-value')}.textContent === 'Включён'`);
  // An untrusted name is always text, including after push updates.
  await evaluate(`${win}.__panelTest.update({rev:99,title:'<img src=x onerror=alert(1)>',eyebrow:'Ш'.repeat(80)})`);
  assert.equal(await evaluate(`${doc}.querySelector('img')`), null);
  await evaluate(`document.querySelector('[data-width="200"]').click()`);
  assert.ok(await evaluate(`${doc}.documentElement.scrollWidth <= ${doc}.documentElement.clientWidth`));
  await screen('device');
  await evaluate(`${win}.__panelTest.deny=true`); await click('on');
  await until(`${get('title')}.textContent === 'Доступ закрыт'`);
  assert.equal(await evaluate(`${get('items')}.textContent`), '');
  assert.equal(await evaluate(`${get('hero-value')}.textContent`), '');
  // The safe recovery after failed mode delivery also handles push-before-answer.
  await screen('settings');
  await evaluate(`${win}.TelegramWidget.request=()=>new Promise(resolve=>{${win}.__resolve=resolve})`);
  await click('classic');
  await evaluate(`${win}.__panelTest.update({rev:2});${win}.__resolve({result:JSON.stringify({transition:true})})`);
  await until(`${get('panel-refresh')}.getAttribute('aria-disabled') === 'false'`);
  await screen('settings');
  await evaluate(`${win}.TelegramWidget.request=async()=>({result:JSON.stringify({transition:true})})`);
  await click('classic'); await until(`${get('feedback')}.textContent.includes('Открываем')`);
  await evaluate(`${win}.__panelTest.update({rev:2})`);
  assert.equal(await evaluate(`${get('panel-refresh')}.getAttribute('aria-disabled')`), 'false');
  // A forwarded card and a standalone payload cannot expose private contents.
  await screen('home');
  await evaluate(`{const f=document.getElementById('panel-frame');f.srcdoc=f.srcdoc.replace('isForward: false','isForward: true')}`);
  await until(`${get('title')} && ${get('title')}.textContent === 'Доступ закрыт'`);
  assert.equal(await evaluate(`${get('items')}.textContent`), '');
  await screen('home');
  await evaluate(`{const f=document.getElementById('panel-frame');f.srcdoc=f.srcdoc.replace(/<script>[\\s\\S]*?<\\/script>/,'')}`);
  await until(`${get('title')} && ${get('title')}.textContent === 'Ожидание данных'`);
  assert.equal(await evaluate(`${get('items')}.textContent`), '');
  await command('Emulation.setDeviceMetricsOverride', {width:390,height:1200,deviceScaleFactor:1,mobile:true});
  await evaluate(`document.querySelector('[data-width="320"]').click();document.querySelector('[data-theme="light"]').click()`);
  await screen('home'); await screenshot('panel-mobile');
  assert.ok(await evaluate('document.documentElement.scrollWidth <= innerWidth'), 'Mobile shell overflow');
  console.log('Panel checks passed: five pages, classic/HTML roundtrip, shared state, duplicate taps, error recovery, revocation, transition races, both themes and 200–430px.');
}
