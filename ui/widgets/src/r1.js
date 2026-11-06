import { validGraph, renderGraph } from './graph-render.js';
// All actions are server-issued opaque keys. This file never interprets a device command.
(() => {
  const api = globalThis.TelegramWidget, el = id => document.getElementById(id), card = el('release');
  const words = {
    ru: ['Назад', 'Далее', 'Обновить', 'Обновление…', 'Нет ответа. Обновите.', 'Доступ закрыт', 'Откройте /classic.', GRAPH ? 'Страница' : '', 'Часть'],
    en: ['Back', 'Next', 'Refresh', 'Updating…', 'No response. Refresh.', 'Access denied', 'Open /classic.', GRAPH ? 'Page' : '', 'Part']
  };
  let data = null, busy = false, waiting = false, uncertain = false, denied = Boolean(api && api.isForward);
  let local = 0, deck = [], timer, requestSeq = -1, pendingRead = false, note = '', activeKey = '';
  let frame = 0, painted = 0;
  let lang = typeof navigator !== 'undefined' && navigator.language.startsWith('ru') ? 'ru' : 'en';
  const w = n => words[lang][n], text = (id, value) => { el(id).textContent = value; };
  const node = (tag, style = '', value = '') => {
    const n = document.createElement(tag); n.className = style; n.textContent = value; return n;
  };
  const hide = (id, value) => el(id).classList.toggle('r-hidden', value);
  const feedback = value => { text('r-feedback', value); hide('r-feedback', !value); };
  const string = value => typeof value === 'string';
  const list = (value, max, test) => Array.isArray(value) && value.length <= max && value.every(test);
  const key = value => string(value) && /^[A-Za-z0-9_-]{1,28}$/.test(value);
  function valid(v) {
    return v && v.v === 1 && string(v.rev) && /^[a-f0-9]{32}$/.test(v.rev) && Number.isSafeInteger(v.seq) && v.seq >= 0
      && ['ru', 'en'].includes(v.lang) && ['title', 'context', 'body', 'notice', 'alert'].every(k => string(v[k]))
      && (v.kind === undefined || (GRAPH ? v.kind === 'list' : string(v.kind)))
      && (GRAPH || v.status === undefined || list(v.status, 2, s => list(s, 3, string) && s.length === 3))
      && (v.hint === undefined || string(v.hint))
      && (v.back == null || key(v.back))
      && (!GRAPH || validGraph(v.graph))
      && v.read === 'read' && [v.prev, v.next].every(k => k === null || key(k))
      && Number.isSafeInteger(v.page) && Number.isSafeInteger(v.pages) && v.page > 0 && v.page <= v.pages
      && list(v.rows, GRAPH ? 4 : 64, row => list(row, GRAPH ? 3 : 2,
        b => b && string(b.text) && key(b.key) && (GRAPH ? ['action', 'nav', 'danger', 'selected'] : ['action', 'nav', 'danger', 'selected', 'wait', 'uncertain', 'status']).includes(b.kind)) && row.length > 0);
  }
  function accept(v) {
    if (v && v.b !== undefined) {
      if (v.rows !== undefined) return false;
      v.rows = v.b.map(row => row.map(b => Array.isArray(b) && b.length === 3 ? {text:b[0], key:b[1], kind:b[2]} : null));
    }
    if (!valid(v)) return false;
    if (data && v.seq < data.seq) return true;
    if (!data || !v.s || v.s !== data.s || v.page !== data.page) local = 0;
    data = v; lang = v.lang;
    return true;
  }
  function disable(node, value) {
    node.setAttribute('aria-disabled', String(value));
  }
  const blocked = read => denied || !data || !api || busy || waiting || (!read && uncertain);
  function action(b) {
    const awaiting = !GRAPH && (b.kind === 'wait' || b.key === activeKey && (busy || waiting));
    const unavailable = !GRAPH && (awaiting || b.kind === 'uncertain');
    const button = node('button', 'r-button r-' + b.kind + (awaiting ? ' r-wait' : ''));
    if (!GRAPH) {
      button.setAttribute('aria-busy', String(awaiting));
    }
    button.setAttribute('data-key', b.key);
    if (b.kind === 'selected') button.setAttribute('aria-pressed', 'true');
    button.appendChild(node('span', '', b.text));
    if (!GRAPH && awaiting) button.appendChild(node('i'));
    disable(button, unavailable || blocked(b.key === data.read));
    button.addEventListener('click', () => !unavailable && send(b.key));
    return button;
  }
  function mount(nodes) {
    el('r-content').textContent = '';
    if (GRAPH) nodes.forEach(node => el('r-content').appendChild(node));
    else {
      let previous;
      nodes.forEach(block => {
      if (block.tagName === 'P') {
        if (!previous || previous.className !== block.className) {
          previous = node('p', block.className);
          el('r-content').appendChild(previous);
        }
        previous.textContent += block.textContent;
      } else { previous = null; el('r-content').appendChild(block); }
      });
    }
  }
  function layout() {
    const blocks = [];
    let statuses;
    const paragraphs = [[data.alert, 'r-alert'], [data.notice, 'r-notice'], [data.body, 'r-body'], [data.hint || '', 'r-hint']];
    if (!GRAPH) for (const id of ['r-title', 'r-context']) {
      if (el(id).scrollWidth > el(id).clientWidth) paragraphs.unshift([el(id).textContent, id + '-full']);
    }
    for (const [value, style] of paragraphs) {
      // Bounded, Unicode-aware fragments prefer whitespace near the end. Mount
      // joins adjacent fragments again; pagination never drops source text.
      for (const part of GRAPH ? [value].filter(Boolean) : value.match(/[\s\S]{60,99}\s|[\s\S]{1,100}/gu) || []) {
        blocks.push(node('p', 'r-text ' + style, part));
      }
    }
    if (!GRAPH && data.status?.length) {
      statuses = node('div', 'r-statuses');
      data.status.forEach(status => {
        const tile = node('div', 'r-button r-status');
        status.forEach((value, i) => tile.appendChild(node(['span', 'strong', 'small'][i], '', value)));
        statuses.appendChild(tile);
      });
      blocks.push(statuses);
    }
    data.rows.forEach(row => {
      row = row.filter(b => b.key !== data.back);
      if (!row.length) return;
      if (!GRAPH && statuses && row[0].kind === 'status') {
        row.forEach(b => statuses.appendChild(action(b))); return;
      }
      const group = node('div', 'r-row');
      row.forEach(b => group.appendChild(action(b))); blocks.push(group);
    });
    mount(blocks); hide('r-local', true);
    if (card.scrollHeight <= 600) { deck = [blocks]; local = 0; return; }
    hide('r-local', false); text('r-local-count', w(8) + ' 1/1');
    deck = []; let part = [];
    for (const block of blocks) {
      part.push(block); mount(part);
      if (card.scrollHeight > 600) {
        part.pop();
        if (part.length) deck.push(part);
        part = [block]; mount(part);
        if (card.scrollHeight > 600) return true;
      }
    }
    if (part.length || !deck.length) deck.push(part);
    local = Math.min(local, deck.length - 1);
    mount(deck[local]);
    hide('r-local', deck.length <= 1);
    text('r-local-count', w(8) + ' ' + (local + 1) + '/' + deck.length);
    disable(el('r-local-prev'), local === 0);
    disable(el('r-local-next'), local === deck.length - 1);
  }
  function render() {
    const locked = denied || !data || !api;
    if (!GRAPH) card.setAttribute('data-kind', locked ? 'menu' : data.kind || 'menu');
    text('r-context', locked ? '' : data.context);
    text('r-title', locked ? w(5) : data.title);
    el('r-read').setAttribute('aria-label', w(!GRAPH && (busy || waiting) ? 3 : 2));
    if (!GRAPH) text('r-read', busy || waiting ? '…' : '↻');
    text('r-back', '‹ ' + w(0));
    hide('r-back', locked || !data.back);
    feedback(locked ? w(6) : note || (!GRAPH && data.rows.flat().some(b => b.kind === 'uncertain') ? w(4) : ''));
    if (GRAPH) renderGraph(locked ? null : data.graph, lang, el, hide);
    ['r-prev', 'r-local-prev'].forEach(id => el(id).setAttribute('aria-label', w(0)));
    ['r-next', 'r-local-next'].forEach(id => el(id).setAttribute('aria-label', w(1)));
    hide('r-server', locked || data.pages <= 1); hide('r-read', locked); hide('r-local', true);
    mount([]);
    if (locked) return;
    text('r-count', (GRAPH ? w(7) + ' ' : '') + data.page + '/' + data.pages);
    ['read', 'prev', 'next', 'back'].forEach(k => disable(el('r-' + k), !data[k] || blocked(k === 'read')));
    if (layout()) {
      mount([]); hide('r-local', true); hide('r-server', true);
      feedback(w(6));
    }
    if (!GRAPH && !frame) frame = requestAnimationFrame(spin);
  }
  // Update a real element so progress does not rely on native CSS animation
  // playback. One frame loop serves all currently mounted rings.
  let reduced = false;
  if (!GRAPH) {
    const motion = matchMedia('(prefers-reduced-motion:reduce)');
    reduced = motion.matches;
    motion.addEventListener('change', event => { reduced = event.matches; });
  }
  const spin = GRAPH ? null : now => {
    frame = 0;
    const rings = card.querySelectorAll('.r-wait i');
    if (!rings.length) return;
    if (now - painted >= 32 && !reduced) {
      rings.forEach(ring => { ring.style.transform = 'rotate(' + now % 1000 * .36 + 'deg)'; });
      painted = now;
    }
    frame = requestAnimationFrame(spin);
  };
  function stopWait() { waiting = false; clearTimeout(timer); }
  function complete() { if (pendingRead) uncertain = false; pendingRead = false; stopWait(); note = ''; }
  function fail() { pendingRead = false; stopWait(); uncertain = true; note = w(4); }
  async function send(k) {
    if (!key(k) || blocked(k === (data && data.read))) return;
    const rev = data.rev, seq = data.seq, read = k === data.read;
    requestSeq = seq; pendingRead = read; activeKey = k; busy = true; note = ''; render();
    try {
      const result = await api.request('r1:' + rev + ':' + k);
      const answer = JSON.parse(result.result);
      if (answer && answer.denied) { denied = true; data = null; stopWait(); }
      else if (answer && answer.page) {
        if (!accept(answer.page)) throw Error();
        if (answer.page.seq > seq) complete();
        else { pendingRead = false; stopWait(); note = uncertain ? w(4) : ''; }
      } else if (answer && answer.accepted) {
        if (data && data.seq > seq) complete();
        else {
          waiting = true;
          timer = setTimeout(() => { fail(); render(); }, 12000);
        }
      } else throw Error();
    } catch { fail(); }
    busy = false; render();
  }
  function update() {
    if (denied) return;
    try {
      if (!accept(JSON.parse(api.chatData))) throw Error();
      if (waiting && data.seq > requestSeq) complete();
    } catch { denied = true; data = null; stopWait(); }
    render();
  }
  function theme() {
    document.documentElement.setAttribute('data-theme', api && api.colorScheme === 'dark' ? 'dark' : 'light');
  }
  ['read', 'prev', 'next', 'back'].forEach(k => el('r-' + k).addEventListener('click', () => data && data[k] && send(data[k])));
  el('r-local-prev').addEventListener('click', () => { if (local > 0) { local--; render(); } });
  el('r-local-next').addEventListener('click', () => { if (local + 1 < deck.length) { local++; render(); } });
  if (api && !denied) {
    api.onEvent('chatDataChanged', update);
    api.onEvent('contentSafeAreaChanged', render);
    api.onEvent('themeChanged', () => { theme(); render(); });
    update(); api.ready();
  }
  if (typeof ResizeObserver === 'function') {
    let width = 0;
    new ResizeObserver(() => {
      const next = card.getBoundingClientRect().width;
      if (next !== width) { width = next; render(); }
    }).observe(card);
  }
  theme(); render();
})();
