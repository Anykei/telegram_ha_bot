import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../src/switch.js', import.meta.url), 'utf8');
const snapshot = (extra = {}) => ({ v: 1, id: '42', rev: 1, name: 'Торшер', room: 'Гостиная', state: 'off', access: 'control', critical: false, ...extra });
function harness(initial = snapshot(), options = {}) {
  const elements = new Map(), requests = [], listeners = {}, timers = new Map();
  let next = 0;
  function element(id) {
    if (!elements.has(id)) {
      const classes = new Set(), handlers = {}, attributes = {};
      elements.set(id, { textContent: '', attributes, handlers, classes,
        classList: { toggle(name, on) { if (on) classes.add(name); else classes.delete(name); } },
        setAttribute(key, value) { attributes[key] = value; },
        addEventListener(key, fn) { handlers[key] = fn; },
      });
    }
    return elements.get(id);
  }
  const api = {
    chatData: JSON.stringify(initial), colorScheme: 'light', isForward: options.forward || false,
    onEvent(key, fn) { listeners[key] = fn; }, ready() {},
    request(command) { return new Promise((resolve, reject) => requests.push({ command, resolve, reject })); },
  };
  vm.runInNewContext(source, {
    document: { getElementById: element, documentElement: element('html') },
    TelegramWidget: options.noBridge ? undefined : api, TextEncoder,
    setTimeout(fn) { timers.set(++next, fn); return next; }, clearTimeout(id) { timers.delete(id); },
  });
  return {
    element, requests, api,
    click(id) { element(id).handlers.click(); },
    async reply(state, extra = {}, index = requests.length - 1) {
      requests[index].resolve({ result: JSON.stringify({ state, ...extra }) });
      await new Promise(resolve => setImmediate(resolve));
    },
    async fail(code) {
      requests.at(-1).reject({ code });
      await new Promise(resolve => setImmediate(resolve));
    },
    update(state) { api.chatData = JSON.stringify(state); listeners.chatDataChanged(); },
    expire() { for (const fn of [...timers.values()]) fn(); timers.clear(); },
  };
}

test('explicit on command is sent once; UI changes only after a confirmed state', async () => {
  const h = harness();
  h.click('power'); h.click('power'); h.click('refresh');
  assert.equal(h.requests.length, 1);
  assert.equal(h.requests[0].command, 'w1:42:1:on');
  assert.equal(h.element('state').textContent, 'Выключен');
  assert.equal(h.element('power').attributes['aria-pressed'], 'false');
  await h.reply(snapshot({ rev: 2, state: 'on' }));
  assert.equal(h.element('state').textContent, 'Включён');
  h.click('power');
  assert.equal(h.requests[1].command, 'w1:42:2:off');
});

test('timeout never retries control; refresh is read-only and unblocks after reconciliation', async () => {
  const h = harness(); h.click('power'); await h.fail('TIMEOUT'); h.click('power');
  assert.equal(h.requests.length, 1);
  assert.equal(h.element('state').textContent, 'Выключен');
  h.click('refresh');
  assert.equal(h.requests[1].command, 'w1:42:1:read');
  await h.reply(snapshot({ rev: 2, state: 'on' }));
  h.click('power');
  assert.equal(h.requests[2].command, 'w1:42:2:off');
});

test('accepted command without observed change keeps the old state and requires refresh', async () => {
  const h = harness(); h.click('power'); await h.reply(snapshot(), { pending: true });
  assert.equal(h.element('state').textContent, 'Выключен');
  h.click('power'); assert.equal(h.requests.length, 1);
  h.update(snapshot({ rev: 2, state: 'on' }));
  assert.equal(h.element('state').textContent, 'Включён');
  h.click('power'); assert.equal(h.requests.length, 2);
});

test('readonly and unavailable cards can refresh but cannot control', () => {
  for (const state of [snapshot({ access: 'view' }), snapshot({ state: 'unavailable' })]) {
    const h = harness(state); h.click('power'); assert.equal(h.requests.length, 0);
    h.click('refresh'); assert.equal(h.requests[0].command, 'w1:42:1:read');
  }
});

test('missing bridge, invalid data and forwards never expose controls or private names', () => {
  for (const h of [harness(null), harness(snapshot(), { noBridge: true }), harness(snapshot(), { forward: true })]) {
    h.click('power'); h.click('refresh');
    assert.equal(h.requests.length, 0);
    assert.equal(h.element('name').textContent, 'Устройство');
  }
});

test('critical actions use a server-issued confirmation; cancel and expiry cannot send it', async () => {
  const h = harness(snapshot({ critical: true })); h.click('power');
  await h.reply(snapshot({ critical: true }), { confirm: { token: 'opaque', action: 'on', expires: Date.now() + 30000 } });
  assert.equal(h.element('power').classes.has('is-hidden'), true);
  assert.equal(h.element('widget').classes.has('is-confirming'), true);
  h.click('cancel'); h.click('confirm'); assert.equal(h.requests.length, 1);
  h.click('power');
  await h.reply(snapshot({ critical: true }), { confirm: { token: 'opaque2', action: 'on', expires: Date.now() + 30000 } });
  h.expire(); h.click('confirm'); assert.equal(h.requests.length, 2);
  h.click('power');
  await h.reply(snapshot({ critical: true }), { confirm: { token: 'opaque3', action: 'on', expires: Date.now() + 30000 } });
  h.click('confirm'); assert.equal(h.requests.at(-1).command, 'w1:42:1:confirm:opaque3');
});

test('revocation during a request hides data, and a delayed old response cannot restore it', async () => {
  const h = harness(); h.click('power'); h.update(snapshot({ rev: 3, access: 'denied' }));
  await h.reply(snapshot({ rev: 2, state: 'on' }));
  assert.equal(h.element('name').textContent, 'Устройство');
  assert.equal(h.element('state').textContent, 'Доступ закрыт');
  h.click('power'); h.click('refresh'); assert.equal(h.requests.length, 1);
});

test('newer permissions invalidate a pending confirmation', async () => {
  const h = harness(snapshot({ critical: true })); h.click('power');
  await h.reply(snapshot({ critical: true }), { confirm: { token: 'opaque', action: 'on', expires: Date.now() + 30000 } });
  h.update(snapshot({ rev: 2, access: 'view', critical: true })); h.click('confirm');
  assert.equal(h.requests.length, 1);
  assert.equal(h.element('confirm-panel').classes.has('is-hidden'), true);
});

test('device metadata is inserted as text and a different target is rejected', async () => {
  const name = '<img src=x onerror=alert(1)>';
  const h = harness(snapshot({ name }));
  assert.equal(h.element('name').textContent, name);
  h.click('power'); await h.reply(snapshot({ id: '43', rev: 2, state: 'on' }));
  assert.equal(h.element('name').textContent, name);
  assert.equal(h.element('state').textContent, 'Выключен');
});
