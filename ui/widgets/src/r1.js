import { validGraph, renderGraph } from './graph-render.js';
// All actions are server-issued opaque keys. This file never interprets a device command.
(() => {
  const api = window.TelegramWidget, el = id => document.getElementById(id), card = el('release');
  const words = {
    ru: ['Назад', 'Далее', 'Обновить', 'Обновление…', 'Нет ответа. Обновите.', 'Доступ закрыт', '/start', GRAPH ? 'Ещё' : 'Отмена', 'Часть'],
    en: ['Back', 'Next', 'Refresh', 'Updating…', 'No response. Refresh.', 'Access denied', '/start', GRAPH ? 'More' : 'Cancel', 'Part']
  };
  let data = null, busy = false, waiting = false, uncertain = false, denied = !api || api.isForward;
  let local = 0, deck = [], timer, requestSeq = -1, pendingRead = false, note = '', activeKey = '';
  let frame = 0, painted = 0, tail = false;
  let lang = navigator.language.startsWith('ru') ? 'ru' : 'en';
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
      && (v.q === undefined || Array.isArray(v.q) && v.q.length === 3 && string(v.q[0]) && v.q.slice(1).every(k => k === null || key(k)))
      && (v.l === undefined || v.l === true)
      && (v.back == null || key(v.back))
      && (!GRAPH || validGraph(v.graph))
      && v.read === 'read' && [v.prev, v.next].every(k => k === null || key(k))
      && Number.isSafeInteger(v.page) && Number.isSafeInteger(v.pages) && v.page > 0 && v.page <= v.pages
      && list(v.rows, GRAPH ? 4 : 64, row => list(row, GRAPH ? 4 : 2,
        b => b && string(b.text) && key(b.key) && (GRAPH ? ['action', 'nav', 'danger', 'selected'] : ['action', 'nav', 'danger', 'selected', 'wait', 'uncertain', 'status']).includes(b.kind)) && row.length > 0);
  }
  function accept(v) {
    if (v && v.b !== undefined) {
      if (v.rows !== undefined) return false;
      v.rows = v.b.map(row => row.map(b => Array.isArray(b) && b.length === 3 ? {text:b[0], key:b[1], kind:b[2]} : null));
    }
    if (!valid(v)) return false;
    if (data && v.seq < data.seq) return true;
    if (!data || !v.s || v.s !== data.s || v.page !== data.page) { local = tail ? 9999 : 0; tail = false; }
    data = v; lang = v.lang;
    return true;
  }
  function disable(node, value) {
    node.setAttribute('aria-disabled', String(value));
  }
  const blocked = read => denied || !data || busy || waiting || (!read && uncertain);
  const external = i => data[i ? 'next' : 'prev'] || data.q?.[i+1];
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
    const content = el('r-content');
    content.textContent = '';
    let previous;
    nodes.forEach(block => {
      const paragraph = !GRAPH && block.tagName === 'P';
      if (!paragraph || !previous || previous.className !== block.className) {
        previous = paragraph ? node('p', block.className) : block;
        content.appendChild(previous);
      }
      if (paragraph) previous.textContent += block.textContent;
    });
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
      for (const part of GRAPH ? [value].filter(Boolean) : value.match(/.{60,99}\s|.{1,100}/gsu) || []) {
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
    deck = []; let part = []; pager();
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
    pager();
  }
  function pager() {
    const q = data.q;
    hide('r-server', !q && deck.length === 1 && data.pages <= 1);
    // The last-part sentinel must not widen the counter while measuring rows.
    text('r-count', (q ? q[0] + '\n' : '') + w(8) + ' ' + data.page + '.' + (local % 9999 + 1));
    ['prev','next'].forEach((key,i) => disable(el('r-'+key), blocked(false) || (i ? local === deck.length-1 : !local) && !external(i)));
  }
  function render() {
    const locked = denied || !data;
    if (!GRAPH) card.setAttribute('data-kind', locked ? 'menu' : data.kind || 'menu');
    card.classList.toggle('r-bounded', !locked && (data.l || data.kind === 'room'));
    const area = api?.contentSafeAreaInset;
    const corners = area && area.cornerLeft + area.cornerRight;
    // Graphs keep Refresh at the top: two 44px arrows and a short page count
    // fit beside wider timestamps, including an edited mark, on 320px cards.
    card.classList.toggle('r-corner', !locked && corners > 0 && card.clientWidth - corners >= (GRAPH ? 200 : 256));
    // Resolve the graph's bottom inset in the same pass as its corner layout.
    // Use a px longhand rather than overriding the env()/max() declaration
    // through a second class rule in the native renderer's cascade.
    if (GRAPH) card.style.paddingBottom = card.classList.contains('r-corner') ? '16px' : '';
    text('r-context', locked ? '' : data.context);
    text('r-title', locked ? w(5) : data.title);
    el('r-read').setAttribute('aria-label', w(!GRAPH && (busy || waiting) ? 3 : 2));
    if (!GRAPH) text('r-read', busy || waiting ? '…' : '↻');
    text('r-back', '‹ ' + w(!GRAPH && ['form','confirmation'].includes(data?.kind) ? 7 : 0));
    hide('r-back', locked || !data.back);
    feedback(locked ? w(6) : note || (!GRAPH && data.rows.flat().some(b => b.kind === 'uncertain') ? w(4) : ''));
    if (GRAPH) renderGraph(locked ? null : data.graph, lang, el, hide);
    ['r-prev', 'r-next'].forEach((id, i) => el(id).setAttribute('aria-label', w(i)));
    hide('r-server', locked || data.pages <= 1); hide('r-read', locked);
    if (GRAPH) {
      hide('r-prev', !!data?.graph); hide('r-count', !!data?.graph);
      text('r-next', data?.graph ? w(7) : '›');
    }
    mount([]);
    if (locked) return;
    ['read', 'back'].forEach(k => disable(el('r-' + k), !data[k] || blocked(k === 'read')));
    if (layout()) {
      mount([]); hide('r-server', true);
      feedback(w(6));
    }
    if (!GRAPH && !frame) frame = requestAnimationFrame(spin);
  }
  // Update a real element so progress does not rely on native CSS animation
  // playback. One frame loop serves all currently mounted rings.
  const motion = matchMedia('(prefers-reduced-motion:reduce)');
  const spin = GRAPH ? null : now => {
    frame = 0;
    const rings = card.querySelectorAll('.r-wait i');
    if (!rings.length) return;
    if (now - painted >= 32 && !motion.matches) {
      rings.forEach(ring => { ring.style.transform = 'rotate(' + now % 1000 * .36 + 'deg)'; });
      painted = now;
    }
    frame = requestAnimationFrame(spin);
  };
  function stopWait() { waiting = false; clearTimeout(timer); }
  function close() {
    if (denied) return;
    denied = true; data = null; stopWait();
    api.request('r1:home').catch(() => {});
  }
  function complete() { if (pendingRead) uncertain = false; pendingRead = false; stopWait(); note = ''; }
  function fail() { pendingRead = false; tail = false; stopWait(); uncertain = true; note = w(4); }
  async function send(k, last = false) {
    if (!key(k) || blocked(k === (data && data.read))) return;
    const rev = data.rev, seq = data.seq, read = k === data.read;
    requestSeq = seq; pendingRead = read; activeKey = k; tail = last; busy = true; note = ''; render();
    try {
      const result = await api.request('r1:' + rev + ':' + k);
      const answer = JSON.parse(result.result);
      if (answer && answer.denied) close();
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
    } catch { close(); }
    render();
  }
  function theme() {
    document.documentElement.setAttribute('data-theme', api && api.colorScheme === 'dark' ? 'dark' : 'light');
  }
  ['read', 'back'].forEach(k => el('r-' + k).addEventListener('click', () => data && data[k] && send(data[k])));
  [el('r-prev'),el('r-next')].forEach((button,i) => button.addEventListener('click', () => {
    if (blocked(false)) return;
    const next = local + (i ? 1 : -1);
    if (next >= 0 && next < deck.length) { local = next; render(); }
    else send(external(i), !i);
  }));
  if (api && !denied) {
    api.onEvent('chatDataChanged', update);
    api.onEvent('contentSafeAreaChanged', render);
    api.onEvent('themeChanged', () => { theme(); render(); });
    update(); api.ready();
  }
  if (typeof ResizeObserver === 'function') {
    let width = 0;
    new ResizeObserver(() => {
      const next = card.clientWidth;
      if (next !== width) { width = next; render(); }
    }).observe(card);
  }
  theme(); render();
})();
