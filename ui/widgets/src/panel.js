// UI-only transport. The host supplies TelegramWidget and all page data.
(() => {
  const api = globalThis.TelegramWidget;
  const el = id => document.getElementById(id);
  const actions = ['home', 'rooms', 'room', 'device', 'settings', 'classic', 'html', 'on', 'off', 'read'];
  const writes = ['classic', 'html', 'on', 'off'];
  let page = null, busy = false, uncertain = false, transition = false;
  let denied = Boolean(api && api.isForward), note = '';
  const text = (id, value) => { el(id).textContent = value; };
  const hide = (id, value) => el(id).classList.toggle('p-hidden', value);
  const short = (value, max) => typeof value === 'string' && value.length <= max;
  function valid(value) {
    return value && value.v === 1 && Number.isSafeInteger(value.rev) && value.rev >= 0
      && ['home', 'rooms', 'room', 'device', 'settings'].includes(value.screen)
      && short(value.eyebrow, 80) && short(value.title, 80) && short(value.description, 180)
      && (value.hero === null || (value.hero && short(value.hero.label, 80) && short(value.hero.value, 80)))
      && Array.isArray(value.items) && value.items.length <= 4
      && value.items.every(item => item && short(item.label, 80) && short(item.detail, 120)
        && actions.includes(item.action) && typeof item.selected === 'boolean' && typeof item.primary === 'boolean')
      && Array.isArray(value.nav) && value.nav.length <= 3
      && value.nav.every(item => item && short(item.label, 40) && actions.includes(item.action));
  }
  function accept(value) {
    if (!valid(value) || (page && value.rev < page.rev)) return false;
    page = value;
    return true;
  }
  function blocked(action) {
    return !api || !page || denied || busy || transition || (uncertain && writes.includes(action));
  }
  function button(item, compact) {
    const b = document.createElement('button');
    b.setAttribute('type', 'button');
    b.setAttribute('data-action', item.action);
    b.className = compact ? '' : 'p-item';
    const off = blocked(item.action);
    b.classList.toggle('p-disabled', off);
    b.setAttribute('aria-disabled', String(off));
    if (compact) b.textContent = item.label;
    else {
      const power = item.action === 'on' || item.action === 'off';
      b.classList.toggle('p-primary', item.primary);
      b.classList.toggle('p-selected', !power && item.selected);
      b.classList.toggle('p-on', power && item.action === 'off');
      if (power || ['classic', 'html'].includes(item.action)) {
        b.setAttribute('aria-pressed', String(power ? item.action === 'off' : item.selected));
      }
      const copy = document.createElement('span');
      copy.className = 'p-copy';
      const label = document.createElement('span');
      label.className = 'p-label'; label.textContent = item.label;
      const detail = document.createElement('span');
      detail.className = 'p-detail'; detail.textContent = item.detail;
      copy.appendChild(label); copy.appendChild(detail); b.appendChild(copy);
      const mark = document.createElement('span');
      mark.className = power ? 'p-toggle' : 'p-chevron';
      if (power) {
        const knob = document.createElement('span');
        knob.className = 'p-knob'; mark.appendChild(knob);
      } else mark.textContent = item.selected ? '✓' : '›';
      mark.setAttribute('aria-hidden', 'true'); b.appendChild(mark);
    }
    b.addEventListener('click', () => {
      if (!(item.selected && ['classic', 'html'].includes(item.action))) send(item.action);
    });
    return b;
  }
  function render() {
    const locked = denied || !page || !api;
    text('eyebrow', locked ? 'Мой дом' : page.eyebrow);
    text('title', denied ? 'Доступ закрыт' : locked ? 'Ожидание данных' : page.title);
    text('description', locked ? '' : page.description);
    el('panel').setAttribute('data-screen', locked ? 'home' : page.screen);
    el('panel').classList.toggle('p-on', !locked && page.screen === 'device' && page.items.some(item => item.action === 'off'));
    el('panel').classList.toggle('p-error', uncertain || denied);
    hide('hero', locked || !page.hero);
    text('hero-label', !locked && page.hero ? page.hero.label : '');
    text('hero-value', !locked && page.hero ? page.hero.value : '');
    for (const id of ['items', 'nav']) {
      el(id).textContent = '';
      if (!locked) page[id].forEach(item => el(id).appendChild(button(item, id === 'nav')));
      hide(id, locked || page[id].length === 0);
    }
    hide('panel-refresh', locked);
    el('panel-refresh').classList.toggle('p-disabled', blocked('read'));
    el('panel-refresh').setAttribute('aria-disabled', String(blocked('read')));
    text('feedback', denied ? 'Откройте новую карточку у бота.' : note || (!api ? 'Откройте карточку в Telegram.' : !page ? 'Ожидание данных от бота.' : ''));
  }
  async function send(action) {
    if (!actions.includes(action) || blocked(action)) return;
    const requestedRev = page.rev;
    busy = true;
    note = 'Подождите…';
    render();
    try {
      const result = await api.request('p1:' + requestedRev + ':' + action);
      const answer = JSON.parse(result.result);
      if (answer && answer.denied) { denied = true; page = null; }
      else if (answer && answer.transition) {
        transition = page.rev <= requestedRev;
        note = transition ? 'Открываем выбранный вид…' : '';
      } else {
        if (!answer || !accept(answer.page)) throw Error('Invalid page');
        if (action === 'read') uncertain = false;
        note = uncertain ? 'Нажмите «Обновить» для проверки.' : '';
      }
    } catch (error) {
      uncertain = true;
      note = error.code === 'RATE_LIMITED' ? 'Подождите и нажмите «Обновить».' : 'Нет ответа. Нажмите «Обновить».';
    }
    busy = false;
    render();
  }
  function read() {
    try {
      const previous = page && page.rev;
      if (!accept(JSON.parse(api.chatData))) throw Error('Invalid page');
      // A failed mode replacement restores this message with a newer revision.
      if (transition && page.rev > previous) { transition = false; note = ''; }
    } catch {
      uncertain = true; note = 'Нужны актуальные данные.';
    }
    render();
  }
  function theme() {
    document.documentElement.setAttribute('data-theme', api && api.colorScheme === 'dark' ? 'dark' : 'light');
  }
  el('panel-refresh').addEventListener('click', () => send('read'));
  if (api && !denied) {
    api.onEvent('chatDataChanged', read);
    api.onEvent('themeChanged', theme);
    read(); api.ready();
  }
  theme(); render();
})();
