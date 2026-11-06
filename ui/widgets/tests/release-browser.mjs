import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

export async function checkRelease({root, command, evaluate, until, screenshot}) {
  const fixtures = JSON.parse(await readFile(root + 'preview/release-fixtures.json', 'utf8'));
  const doc = `document.getElementById('release-frame').contentDocument`;
  const win = `document.getElementById('release-frame').contentWindow`;
  const get = id => `${doc}.getElementById('${id}')`;
  const button = key => `${doc}.querySelector('[data-key="${key}"]')`;
  const state = `JSON.parse(${win}.TelegramWidget.chatData)`;
  const click = key => evaluate(`${button(key)}.click()`);
  const disabled = key => `${button(key)}.getAttribute('aria-disabled')`;
  async function capture(name) {
    await until(`document.getElementById('release-frame').clientHeight === Math.min(600,Math.ceil(${get('release')}.getBoundingClientRect().height))`);
    await screenshot(name);
  }
  async function reload(id, {theme = 'light', width = 360, scenario = 'ok'} = {}) {
    await evaluate(`new Promise(resolve => {
      const f=document.getElementById('release-frame');
      f.addEventListener('load',()=>resolve(true),{once:true});
      theme=${JSON.stringify(theme)};width=${width};
      document.getElementById('scenario').value=${JSON.stringify(scenario)};
      document.getElementById('fixture').value=${JSON.stringify(id)};
      document.querySelectorAll('[data-theme]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.theme===theme)));
      document.querySelectorAll('[data-width]').forEach(b=>b.setAttribute('aria-pressed',String(Number(b.dataset.width)===width)));
      document.getElementById('fixture').dispatchEvent(new Event('change'));
    })`);
    await until(`${get('release')} && ${get('r-feedback')}`);
  }
  async function dimensions(label) {
    const result = await evaluate(`({
      height:${doc}.body.scrollHeight,
      width:${doc}.documentElement.scrollWidth,
      available:${doc}.documentElement.clientWidth,
      textFits:Array.from(${doc}.querySelectorAll('.r-text')).every(p=>p.scrollWidth<=p.clientWidth),
      targets:Array.from(${doc}.querySelectorAll('button')).map(b=>b.getBoundingClientRect()).filter(r=>r.width).map(r=>({width:r.width,height:r.height}))
    })`);
    assert.ok(result.height <= 600, `${label}: height ${result.height} <=600`);
    assert.ok(result.width <= result.available, `${label}: no horizontal overflow`);
    assert.ok(result.textFits, `${label}: full text wraps without clipping`);
    assert.ok(result.targets.every(r => r.height >= 44 && r.width >= 44), `${label}: touch targets >=44px`);
  }
  async function collectParts(label, page) {
    let text = '', actions = [], part = 0;
    while (true) {
      assert.ok(++part <= 40, `${label}: finite local pagination`);
      await dimensions(`${label}/part${part}`);
      if (page && page.back) {
        const back = await evaluate(`({text:${get('r-back')}.textContent,rect:${get('r-back')}.getBoundingClientRect().toJSON()})`);
        assert.equal(back.text, page.lang === 'ru' ? '‹ Назад' : '‹ Back');
        assert.ok(back.rect.width >= 44 && back.rect.height >= 44 && back.rect.top < 30, `${label}/part${part}: persistent Back at the top`);
      }
      const content = await evaluate(`({
        text:Array.from(${doc}.querySelectorAll('.r-text')).map(n=>n.textContent).join(''),
        actions:Array.from(${doc}.querySelectorAll('[data-key]')).map(b=>[b.dataset.key,b.firstChild.textContent]),
        last:${get('r-local')}.classList.contains('r-hidden') || ${get('r-local-next')}.getAttribute('aria-disabled')==='true'
      })`);
      text += content.text; actions.push(...content.actions);
      if (content.last) break;
      await evaluate(`${get('r-local-next')}.click()`);
    }
    for (let i = 1; i < part; i++) await evaluate(`${get('r-local-prev')}.click()`);
    return {text, actions, parts: part};
  }
  async function assertPreserved(page, label) {
    const content = await collectParts(label, page);
    assert.ok(content.text.endsWith(page.alert + page.notice + page.body + (page.hint || '')), `${label}: all body/hint text survives local pagination exactly`);
    assert.deepEqual(content.actions, page.rows.flat().map(b => [b.key, b.text]), `${label}: every original action survives in order`);
    assert.equal(await evaluate(`${get('r-title')}.textContent`), page.title);
    assert.equal(await evaluate(`${get('release')}.getAttribute('data-kind')`), page.kind || 'menu');
    assert.equal(await evaluate(`${get('r-read-label')}.textContent`), page.lang === 'ru' ? 'Обновить' : 'Refresh');
    return content;
  }
  async function controlRequests() {
    await evaluate(`{
      const w=${win};w.__requests=[];
      w.TelegramWidget.request=value=>new Promise((resolve,reject)=>w.__requests.push({value,resolve,reject}));
    }`);
  }
  const reply = value => evaluate(`${win}.__requests.at(-1).resolve({result:JSON.stringify(${JSON.stringify(value)})})`);
  const push = patch => evaluate(`${win}.previewSetData({...${state},...${JSON.stringify(patch)}})`);
  async function failAction() {
    await controlRequests(); await click('rooms');
    await evaluate(`${win}.__requests.at(-1).reject(Error('timeout'))`);
    await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
    assert.equal(await evaluate(disabled('rooms')), 'true');
  }
  await command('Emulation.setDeviceMetricsOverride', {width:1280,height:1080,deviceScaleFactor:1,mobile:false});
  await command('Page.navigate', {url:pathToFileURL(root+'dist/release-preview.html').href});
  await until(`${get('r-title')} && ${get('r-title')}.textContent === 'Мой дом'`);
  let localPaginationSeen = false;
  for (const theme of ['light', 'dark']) {
    for (const width of [200, 320, 360, 430]) {
      for (const fixture of fixtures) {
        await reload(fixture.id, {theme, width});
        assert.equal(await evaluate(`${doc}.documentElement.getAttribute('data-theme')`), theme);
        const content = await assertPreserved(fixture.page, `${fixture.id}/${theme}/${width}`);
        localPaginationSeen ||= content.parts > 1;
      }
    }
  }
  assert.ok(localPaginationSeen, 'Long fixture exercises local pagination');
  // Old snapshots without optional kind/hint still have the default menu presentation.
  await reload('home');
  assert.equal(await evaluate(`${get('release')}.getAttribute('data-kind')`), 'menu');
  assert.equal(await evaluate(`${doc}.querySelector('.r-hint')`), null);
  assert.equal(await evaluate(`${get('r-back')}.classList.contains('r-hidden')`), true);
  await push({back:null});
  assert.equal(await evaluate(`${get('r-back')}.classList.contains('r-hidden')`), true);
  await reload('home'); await capture('release-home-light');
  // Server chunks use the issued keys and update only after a new snapshot.
  await evaluate(`${get('r-next')}.click()`);
  await until(`${get('r-count')}.textContent === 'Страница 2/2'`);
  assert.ok(await evaluate(`${button('help')} && ${button('admin')}`));
  await evaluate(`${get('r-prev')}.click()`);
  await until(`${get('r-count')}.textContent === 'Страница 1/2'`);
  assert.ok(await evaluate(`${button('rooms')}`));
  // Language changes use the same server-issued action path.
  await reload('settings'); await capture('release-settings');
  await click('en'); await until(`${get('r-read-label')}.textContent === 'Refresh'`);
  assert.equal(await evaluate(`${get('r-title')}.textContent`), 'My settings');
  await click('ru'); await until(`${get('r-read-label')}.textContent === 'Обновить'`);
  await reload('room', {theme:'dark'}); await capture('release-room-dark');
  // The name/state opens a device card, while its adjacent command keeps a distinct key.
  assert.equal(await evaluate(`${button('device')}.classList.contains('r-nav')`), true);
  assert.equal(await evaluate(`${button('lamp_on')}.classList.contains('r-action')`), true);
  await click('device'); await until(`${get('r-title')}.textContent === 'Торшер'`);
  await capture('release-device-dark');
  assert.equal(await evaluate(`${get('r-content')}.querySelector('.r-text').textContent`), 'Состояние: Выключен.');
  await click('room'); await until(`${get('r-title')}.textContent === 'Гостиная'`);
  await click('ceiling'); await until(`${get('r-title')}.textContent === 'Люстра'`);
  assert.equal(await evaluate(`${get('r-content')}.querySelector('.r-text').textContent`), 'Состояние: Включена.');
  for (const [id, theme] of [['scenes','light'],['confirmation','light'],['climate','dark'],['form','light'],['admin','light']]) {
    await reload(id, {theme}); await capture('release-'+id+'-'+theme);
  }
  // The thermostat's original Back lives on chunk 2; its persistent control works on both.
  for (const lang of ['ru','en']) for (const theme of ['light','dark']) for (const serverPage of [1,2]) {
    await reload(lang === 'ru' ? 'climate' : 'climate_en', {theme,width:200});
    if (serverPage === 2) {
      await evaluate(`${get('r-next')}.click()`);
      await until(`${get('r-count')}.textContent.endsWith(' 2/2') && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    }
    assert.equal(await evaluate(`${get('r-back')}.textContent`), lang === 'ru' ? '‹ Назад' : '‹ Back');
    assert.equal(await evaluate(`${get('r-back')}.getAttribute('aria-disabled')`), 'false');
    await evaluate(`${get('r-back')}.click()`);
    await until(`${get('r-title')}.textContent === ${JSON.stringify(lang === 'ru' ? 'Гостиная' : 'Living room')} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
  }
  // Returning from each local part dispatches exactly one canonical action, not pagination.
  for (const id of ['confirmation','confirmation_en','long']) for (const theme of ['light','dark']) {
    const fixture = fixtures.find(f=>f.id===id);
    await reload(id, {theme,width:200});
    const {parts} = await collectParts('persistent back/'+id, fixture.page);
    for (let part=0;part<parts;part++) {
      await reload(id, {theme,width:200});
      for (let i=0;i<part;i++) await evaluate(`${get('r-local-next')}.click()`);
      await controlRequests();
      await evaluate(`${get('r-back')}.click();${get('r-back')}.click()`);
      assert.equal(await evaluate(`${win}.__requests.length`), 1);
      assert.equal(await evaluate(`${win}.__requests[0].value`), `r1:${fixture.page.rev}:${fixture.page.back}`);
      assert.equal(await evaluate(`${get('r-title')}.textContent`), fixture.page.title);
      assert.equal(await evaluate(`${get('r-back')}.getAttribute('aria-disabled')`), 'true');
      await reply({accepted:true});
      const destination = fixtures.find(f=>f.id === fixture.page.back + (fixture.page.lang==='en' && fixture.page.back==='scenes' ? '_en' : ''));
      await evaluate(`${win}.previewSetData(${JSON.stringify({...destination.page,seq:100,rev:'a'.repeat(32)})})`);
      await until(`${get('r-title')}.textContent === ${JSON.stringify(destination.page.title)} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    }
  }
  // Malformed keys fail before request; unknown well-formed keys are rejected by the server map.
  await reload('climate'); await controlRequests(); await push({back:'bad:key'});
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  await evaluate(`${get('r-back')}.click()`);
  assert.equal(await evaluate(`${win}.__requests.length`), 0);
  await reload('climate'); await push({back:'unknown_key'});
  await evaluate(`${get('r-back')}.click()`);
  await until(`${get('r-title')}.textContent === 'Доступ закрыт'`);
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  // A failed Back follows ordinary recovery and never retries or bypasses the lock.
  await reload('climate', {scenario:'error'}); await evaluate(`${get('r-back')}.click()`);
  await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
  assert.equal(await evaluate(`${get('r-back')}.getAttribute('aria-disabled')`), 'true');
  await evaluate(`${get('r-read')}.click()`);
  await until(`${get('r-back')}.getAttribute('aria-disabled') === 'false' && ${get('r-feedback')}.textContent === ''`);
  // Existing callback menus keep their entire continuation and both directions.
  for (const lang of ['ru','en']) {
    await reload(lang === 'ru' ? 'admin' : 'admin_en');
    for (const page of [2,3]) {
      await evaluate(`${get('r-next')}.click()`);
      await until(`${get('r-count')}.textContent.endsWith(' ${page}/3') && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    }
    for (const page of [2,1]) {
      await evaluate(`${get('r-prev')}.click()`);
      await until(`${get('r-count')}.textContent.endsWith(' ${page}/3') && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    }
    assert.equal(await evaluate(`${get('r-read-label')}.textContent`), lang === 'ru' ? 'Обновить' : 'Refresh');
  }
  // Form prompts preserve every instruction; typing remains in the Telegram chat.
  await reload('form', {width:200});
  const form = fixtures.find(f => f.id === 'form').page;
  await assertPreserved(form, 'form hint/200');
  assert.equal(await evaluate(`${doc}.querySelector('input,textarea,select')`), null);
  // A failed confirmation never retries its action and returns through the original Cancel key.
  for (const lang of ['ru','en']) {
    await reload(lang === 'ru' ? 'confirmation' : 'confirmation_en', {scenario:'error'});
    await click('run_scene');
    await until(`${get('r-feedback')}.textContent.includes(${JSON.stringify(lang === 'ru' ? 'Нет ответа' : 'No response')})`);
    assert.equal(await evaluate(disabled('run_scene')), 'true');
    await evaluate(`${get('r-read')}.click()`);
    await until(`${disabled('scenes')} === 'false' && ${get('r-feedback')}.textContent === ''`);
    await evaluate(`${win}.TelegramWidget.request=async key=>{${win}.__lastRequest=key;return {result:JSON.stringify({accepted:true})}}`);
    await click('scenes');
    assert.ok((await evaluate(`${win}.__lastRequest`)).endsWith(':scenes'));
    const next = {...fixtures.find(f=>f.id === (lang==='ru'?'scenes':'scenes_en')).page,seq:100,rev:'a'.repeat(32)};
    await evaluate(`${win}.previewSetData(${JSON.stringify(next)})`);
    await until(`${get('r-title')}.textContent === ${JSON.stringify(next.title)} && ${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
  }
  // A command timeout blocks every action; read recovers through accepted + push.
  await reload('home', {scenario:'error'}); await click('rooms');
  await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
  assert.equal(await evaluate(disabled('rooms')), 'true');
  await evaluate(`${get('r-read')}.click()`);
  await until(`${get('r-feedback')}.textContent === '' && ${disabled('rooms')} === 'false'`);
  // An ACK without its promised update expires once and never sends an automatic retry.
  await reload('home'); await controlRequests();
  await evaluate(`{
    const w=${win}, original=w.setTimeout;
    w.setTimeout=(fn,delay,...args)=>original(fn,delay===12000?500:delay,...args);
  }`);
  await click('rooms'); await reply({accepted:true});
  await until(`${get('r-feedback')}.textContent.includes('Нет ответа')`);
  assert.equal(await evaluate(`${win}.__requests.length`), 1);
  await push({seq:2,rev:'2'.repeat(32)});
  assert.equal(await evaluate(disabled('rooms')), 'true', 'Late command data does not recover after ACK timeout');
  await evaluate(`${get('r-read')}.click()`); await reply({accepted:true});
  await push({seq:3,rev:'3'.repeat(32)});
  await until(`${disabled('rooms')} === 'false'`);
  // Both message/ACK orders recover, but never before the successful read ACK.
  for (const dataBeforeAck of [false, true]) {
    await reload('home'); await failAction();
    await evaluate(`${get('r-read')}.click()`);
    assert.ok((await evaluate(`${win}.__requests.at(-1).value`)).endsWith(':read'));
    if (dataBeforeAck) {
      await push({seq:10,rev:'a'.repeat(32)});
      assert.equal(await evaluate(disabled('rooms')), 'true');
    }
    await reply({accepted:true});
    if (!dataBeforeAck) {
      assert.equal(await evaluate(disabled('rooms')), 'true');
      await push({seq:10,rev:'a'.repeat(32)});
    }
    await until(`${disabled('rooms')} === 'false' && ${get('r-feedback')}.textContent === ''`);
    await click('rooms');
    assert.equal(await evaluate(`${win}.__requests.at(-1).value`), 'r1:'+'a'.repeat(32)+':rooms');
  }
  // Same/older read replies and unsolicited late data cannot clear a failed command.
  for (const seq of [0, 1]) {
    await reload('home'); await failAction();
    await evaluate(`${get('r-read')}.click()`);
    await reply({page:{...fixtures[0].page,seq}});
    await until(`${get('r-read')}.getAttribute('aria-disabled') === 'false'`);
    assert.equal(await evaluate(disabled('rooms')), 'true');
    assert.ok(await evaluate(`${get('r-feedback')}.textContent.includes('Нет ответа')`));
  }
  await reload('home'); await failAction();
  await push({seq:9,title:'Новый экран',rev:'9'.repeat(32)});
  assert.equal(await evaluate(disabled('rooms')), 'true');
  await push({seq:2,title:'Старый экран',rev:'2'.repeat(32)});
  assert.equal(await evaluate(`${get('r-title')}.textContent`), 'Новый экран');
  await evaluate(`${get('r-read')}.click()`); await reply({accepted:true});
  await push({seq:10,rev:'a'.repeat(32)});
  await until(`${disabled('rooms')} === 'false'`);
  // Only one request may be in flight; confirmed content never changes optimistically.
  await reload('home'); await controlRequests();
  await click('rooms'); await click('rooms'); await evaluate(`${get('r-read')}.click()`);
  assert.equal(await evaluate(`${win}.__requests.length`), 1);
  assert.equal(await evaluate(`${get('r-title')}.textContent`), 'Мой дом');
  await reply({accepted:true});
  assert.equal(await evaluate(disabled('rooms')), 'true');
  // Both denial forms hide private content, including during a pending request.
  await evaluate(`${win}.previewSetData({denied:true})`);
  await until(`${get('r-title')}.textContent === 'Доступ закрыт'`);
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  await reload('home', {scenario:'denied'}); await click('rooms');
  await until(`${get('r-title')}.textContent === 'Доступ закрыт'`);
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  await reload('home', {scenario:'forward'});
  assert.equal(await evaluate(`${get('r-content')}.textContent`), '');
  assert.ok(await evaluate(`${get('r-feedback')}.textContent.includes('/classic')`));
  // Arbitrary strings stay text; long title/context stay available in local parts.
  await reload('home', {width:200});
  const longPage = {...fixtures[0].page,seq:50,title:'<img src=x onerror=alert(1)> '+'Ш'.repeat(80),context:'Г'.repeat(80),hint:'<img src=x onerror=alert(1)>'};
  await evaluate(`${win}.previewSetData(${JSON.stringify(longPage)})`);
  assert.equal(await evaluate(`${doc}.querySelector('img')`), null);
  const longContent = await assertPreserved(longPage, 'long titles/200');
  assert.ok(longContent.text.includes(longPage.title));
  assert.ok(longContent.text.includes(longPage.context));
  // Mobile shell and native widget remain within their separate width constraints.
  await command('Emulation.setDeviceMetricsOverride', {width:390,height:1300,deviceScaleFactor:1,mobile:true});
  await reload('home', {width:320});
  await evaluate(`document.getElementById('chat').scrollIntoView({block:'start'})`);
  await capture('release-mobile');
  await dimensions('mobile');
  assert.ok(await evaluate('document.documentElement.scrollWidth <= innerWidth'), 'Release mobile shell overflow');
  console.log(`Release checks passed: ${fixtures.length} fixtures × 2 themes × 4 widths, persistent Back across server/local pages, all text/actions, separate device navigation/control, RU/EN, timeout/read recovery, both ACK races, stale seq, duplicate taps, denial, forwards and mobile.`);
}
