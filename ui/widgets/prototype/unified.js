// One immutable document. Existing renderers receive a disposable SDK facade;
// navigation swaps only DOM nodes, never the message or browsing context.
import menu from './generated-menu.js';
import media from './generated-media.js';
import assets from './generated-assets.json';

const sdk = window.TelegramWidget;
const host = document.getElementById('u-host');
const style = document.getElementById('u-style');
let active, state = sdk?.chatData, sequence = -1;
const kind = page => page?.kind === 'chart' ? 'graph'
  : ['range', 'calendar', 'time'].includes(page?.kind) ? 'range'
  : page?.v === 1 && typeof page.title === 'string' ? 'release' : active?.mode || 'release';

function activate(raw, recovery=false) {
  let page;
  try { page = JSON.parse(raw); } catch {}
  if (Number.isSafeInteger(page?.seq) && page.seq < sequence) return;
  if (active?.revoked && !recovery) {
    if (page?.kind !== 'home' || !(page.seq > sequence)) return;
    recovery = true;
  }
  if (Number.isSafeInteger(page?.seq)) sequence = page.seq;
  state = raw;
  const mode = kind(page);
  if (!recovery && active?.mode === mode) { active.events.chatDataChanged?.(); return; }
  active?.dispose();
  const timers = new Set(), frames = new Set(), observers = new Set();
  const scope = {mode, events: {}, alive: true, dispose() {
    this.alive = false;
    timers.forEach(id => clearTimeout(id));
    frames.forEach(id => cancelAnimationFrame(id));
    observers.forEach(observer => observer.disconnect());
    this.events = {};
  }};
  active = scope;
  const live = () => scope.alive;
  const runtime = {
    setTimeout(fn, ms) {
      const id = setTimeout(() => { timers.delete(id); if (live()) fn(); }, ms);
      timers.add(id); return id;
    },
    clearTimeout(id) { timers.delete(id); clearTimeout(id); },
    requestAnimationFrame(fn) {
      const id = requestAnimationFrame(now => { frames.delete(id); if (live()) fn(now); });
      frames.add(id); return id;
    },
    cancelAnimationFrame(id) { frames.delete(id); cancelAnimationFrame(id); },
    ResizeObserver: typeof ResizeObserver === 'function' ? class {
      constructor(fn) {
        this.observer = new ResizeObserver(entries => { if (live()) fn(entries); });
        observers.add(this.observer);
      }
      observe(node) { this.observer.observe(node); }
    } : undefined
  };
  const api = sdk && {
    get chatData() { return state; },
    get colorScheme() { return sdk.colorScheme; },
    get isForward() { return sdk.isForward; },
    get contentSafeAreaInset() { return sdk.contentSafeAreaInset; },
    ready() { sdk.ready(); },
    onEvent(name, fn) { scope.events[name] = fn; },
    async request(value) {
      const recovery = value === 'r1:home';
      if (recovery) scope.revoked = true;
      const result = await sdk.request(value);
      // A late ACK belongs to the disposed screen. Its async continuation may
      // finish, but render() is guarded and cannot touch the new screen's DOM.
      if (!live() || scope.revoked && !recovery) throw Error('Stale screen');
      let answer;
      try { answer = JSON.parse(result.result); } catch {}
      if (answer?.page && (recovery || kind(answer.page) !== mode)) activate(JSON.stringify(answer.page), recovery);
      else if (Number.isSafeInteger(answer?.page?.seq)) {
        if (answer.page.seq >= sequence) { sequence = answer.page.seq; state = JSON.stringify(answer.page); }
      }
      return result;
    }
  };
  style.textContent = assets[mode].css;
  host.innerHTML = assets[mode].html;
  if (mode === 'release') menu(api, runtime, live);
  else media(api, runtime, live, mode === 'graph');
}

sdk?.onEvent('chatDataChanged', () => activate(sdk.chatData));
for (const name of ['contentSafeAreaChanged', 'themeChanged']) {
  sdk?.onEvent(name, () => active?.events[name]?.());
}
activate(state);
