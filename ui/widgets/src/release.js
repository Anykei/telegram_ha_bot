// All actions are server-issued opaque keys. This file never interprets a device command.
(() => {
  const api = globalThis.TelegramWidget, el = id => document.getElementById(id), card = el('release');
  const words = {
    ru: ['Предыдущая', 'Далее', 'Обновить', 'Обновляем экран…', 'Нет ответа. Нажмите «Обновить».', 'Доступ закрыт', 'Откройте /classic в личном чате с ботом.', 'Страница', 'Часть', 'Назад'],
    en: ['Previous', 'Next', 'Refresh', 'Updating…', 'No response. Press Refresh.', 'Access denied', 'Open /classic in your private chat with the bot.', 'Page', 'Part', 'Back']
  };
  let data = null, busy = false, waiting = false, uncertain = false, denied = Boolean(api && api.isForward);
  let local = 0, deck = [], timer, requestSeq = -1, pendingRead = false, note = '';
  let lang = typeof navigator !== 'undefined' && navigator.language.startsWith('ru') ? 'ru' : 'en';
  const w = n => words[lang][n], text = (id, value) => { el(id).textContent = value; };
  const hide = (id, value) => el(id).classList.toggle('r-hidden', value);
  const key = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,28}$/.test(value);
  function valid(v) {
    return v && v.v === 1 && typeof v.rev === 'string' && /^[a-f0-9]{32}$/.test(v.rev) && Number.isSafeInteger(v.seq) && v.seq >= 0
      && ['ru', 'en'].includes(v.lang) && ['title', 'context', 'body', 'notice', 'alert'].every(k => typeof v[k] === 'string')
      && (v.kind === undefined || ['menu', 'device', 'climate', 'confirmation', 'form', 'list'].includes(v.kind))
      && (v.hint === undefined || typeof v.hint === 'string')
      && (v.back == null || key(v.back))
      && v.read === 'read' && [v.prev, v.next].every(k => k === null || key(k))
      && Number.isSafeInteger(v.page) && Number.isSafeInteger(v.pages) && v.page > 0 && v.page <= v.pages
      && Array.isArray(v.rows) && v.rows.length <= 4 && v.rows.every(row => Array.isArray(row) && row.length > 0 && row.length <= 2
        && row.every(b => b && typeof b.text === 'string' && key(b.key) && ['action', 'nav', 'danger', 'selected'].includes(b.kind)));
  }
  function accept(v) {
    if (!valid(v)) return false;
    if (data && v.seq < data.seq) return true;
    if (!data || v.seq > data.seq) local = 0;
    data = v; lang = v.lang;
    return true;
  }
  function disable(node, value) {
    node.classList.toggle('r-disabled', value);
    node.setAttribute('aria-disabled', String(value));
  }
  const blocked = read => denied || !data || !api || busy || waiting || (!read && uncertain);
  function action(b) {
    const node = document.createElement('button');
    node.className = 'r-button r-' + b.kind;
    node.setAttribute('type', 'button'); node.setAttribute('data-key', b.key);
    if (b.kind === 'selected') node.setAttribute('aria-pressed', 'true');
    const label = document.createElement('span'); label.textContent = b.text; node.appendChild(label);
    const mark = document.createElement('span'); mark.className = 'r-mark';
    mark.textContent = { nav: '›', selected: '✓', danger: '!', action: '•' }[b.kind];
    mark.setAttribute('aria-hidden', 'true'); node.appendChild(mark);
    disable(node, blocked(b.key === data.read));
    node.addEventListener('click', () => send(b.key));
    return node;
  }
  function mount(nodes) {
    el('r-content').textContent = '';
    let previous;
    nodes.forEach(node => {
      if (node.tagName === 'P') {
        if (!previous || previous.className !== node.className) {
          previous = document.createElement('p'); previous.className = node.className;
          el('r-content').appendChild(previous);
        }
        previous.textContent += node.textContent;
      } else { previous = null; el('r-content').appendChild(node); }
    });
  }
  function layout() {
    const blocks = [];
    const paragraphs = [[data.alert, 'r-alert'], [data.notice, 'r-notice'], [data.body, 'r-body'], [data.hint || '', 'r-hint']];
    for (const id of ['r-title', 'r-context']) {
      if (el(id).scrollWidth > el(id).clientWidth) paragraphs.unshift([el(id).textContent, id + '-full']);
    }
    for (const [value, style] of paragraphs) {
      const chars = Array.from(value);
      while (chars.length) {
        const node = document.createElement('p'); node.className = 'r-text ' + style;
        let end = Math.min(100, chars.length);
        while (end > 60 && end < chars.length && !/\s/.test(chars[end - 1])) end--;
        node.textContent = chars.splice(0, end).join(''); blocks.push(node);
      }
    }
    data.rows.forEach(row => {
      const node = document.createElement('div'); node.className = 'r-row';
      row.forEach(b => node.appendChild(action(b))); blocks.push(node);
    });
    mount(blocks); hide('r-local', true);
    if (card.getBoundingClientRect().height <= 580) { deck = [blocks]; local = 0; return; }
    hide('r-local', false); text('r-local-count', w(8) + ' 1/1');
    deck = []; let part = [];
    for (const block of blocks) {
      part.push(block); mount(part);
      if (card.getBoundingClientRect().height > 580) {
        part.pop();
        if (part.length) deck.push(part);
        part = [block]; mount(part);
        if (card.getBoundingClientRect().height > 580) return true;
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
    card.setAttribute('data-kind', locked ? 'menu' : data.kind || 'menu');
    text('r-context', locked ? '' : data.context);
    text('r-title', locked ? w(5) : data.title);
    text('r-read-label', w(2));
    text('r-back', '‹ ' + w(9));
    hide('r-back', locked || !data.back);
    text('r-feedback', locked ? w(6) : note);
    card.classList.toggle('r-error', uncertain || denied);
    ['r-prev', 'r-local-prev'].forEach(id => el(id).setAttribute('aria-label', w(0)));
    ['r-next', 'r-local-next'].forEach(id => el(id).setAttribute('aria-label', w(1)));
    hide('r-server', locked || data.pages <= 1); hide('r-read', locked); hide('r-local', true);
    mount([]);
    if (locked) return;
    text('r-count', w(7) + ' ' + data.page + '/' + data.pages);
    ['read', 'prev', 'next', 'back'].forEach(k => disable(el('r-' + k), !data[k] || blocked(k === 'read')));
    if (layout()) {
      mount([]); hide('r-local', true); hide('r-server', true);
      text('r-feedback', w(6));
    }
  }
  function stopWait() { waiting = false; clearTimeout(timer); }
  function complete() { if (pendingRead) uncertain = false; pendingRead = false; stopWait(); note = ''; }
  function fail() { pendingRead = false; stopWait(); uncertain = true; note = w(4); }
  async function send(k) {
    if (!key(k) || blocked(k === (data && data.read))) return;
    const rev = data.rev, seq = data.seq, read = k === data.read;
    requestSeq = seq; pendingRead = read; busy = true; note = w(3); render();
    try {
      const result = await api.request('r1:' + rev + ':' + k);
      const answer = JSON.parse(result.result);
      if (answer && answer.denied) { denied = true; data = null; stopWait(); }
      else if (answer && answer.page) {
        if (!accept(answer.page)) throw Error('Invalid page');
        if (answer.page.seq > seq) complete();
        else { pendingRead = false; stopWait(); note = uncertain ? w(4) : ''; }
      } else if (answer && answer.accepted) {
        if (data && data.seq > seq) complete();
        else {
          waiting = true;
          timer = setTimeout(() => { fail(); render(); }, 12000);
        }
      } else throw Error('Invalid response');
    } catch { fail(); }
    busy = false; render();
  }
  function update() {
    if (denied) return;
    try {
      if (!accept(JSON.parse(api.chatData))) throw Error('Invalid data');
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
