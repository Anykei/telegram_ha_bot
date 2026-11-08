// Transport: TelegramWidget only. The preview supplies an in-memory substitute.
(() => {
  const el = id => document.getElementById(id);
  const api = globalThis.TelegramWidget;
  let data = null, busy = false, uncertain = false, confirmation = null, timer;
  let note = '', denied = Boolean(api && api.isForward);
  const text = (id, value) => { el(id).textContent = value; };
  const show = (id, visible) => el(id).classList.toggle('is-hidden', !visible);
  const disable = (id, value) => {
    el(id).classList.toggle('is-disabled', value);
    el(id).setAttribute('aria-disabled', String(value));
  };
  function valid(value) {
    return value && value.v === 1 && /^[0-9]{1,12}$/.test(value.id)
      && Number.isSafeInteger(value.rev) && value.rev >= 0
      && ['on', 'off', 'unavailable'].includes(value.state)
      && ['control', 'view', 'denied'].includes(value.access)
      && typeof value.name === 'string' && value.name.length <= 80
      && typeof value.room === 'string' && value.room.length <= 80
      && typeof value.critical === 'boolean';
  }
  function accept(value) {
    if (!valid(value) || (data && (value.id !== data.id || value.rev < data.rev))) return false;
    if (data && value.rev > data.rev) { confirmation = null; clearTimeout(timer); }
    data = value;
    if (value.access === 'denied') denied = true;
    return true;
  }
  function read() {
    try {
      const revision = data && data.rev;
      if (!accept(JSON.parse(api.chatData))) throw Error();
      if (revision === null || data.rev > revision) uncertain = false;
      note = '';
    } catch {
      uncertain = true;
      note = 'Обновите состояние.';
    }
    render();
  }
  function render() {
    const online = data && data.state !== 'unavailable';
    const on = online && data.state === 'on';
    const locked = denied || !data || !api;
    el('widget').setAttribute('data-state', denied ? 'denied' : !online ? 'offline' : data.state);
    el('widget').classList.toggle('is-busy', busy);
    text('room', locked ? 'Мой дом' : data.room);
    text('name', locked ? 'Устройство' : data.name);
    text('device-id', locked ? '—' : '№ ' + data.id);
    text('state', denied ? 'Доступ закрыт' : !data ? 'Нет данных' : !online ? 'Недоступен' : on ? 'Включён' : 'Выключен');
    text('power-label', busy ? 'Подождите…' : uncertain ? 'Нужно обновление' : data && data.access === 'view' ? 'Только просмотр' : on ? 'Выключить свет' : 'Включить свет');
    el('power').setAttribute('aria-pressed', String(Boolean(on)));
    text('status', denied ? 'Откройте новую карточку у бота.' : note || (!api ? 'Откройте карточку в Telegram.' : !data ? 'Ожидание данных от бота.' : data.access === 'view' ? 'Управление недоступно.' : !online ? 'Нет связи. Попробуйте обновить.' : 'Состояние получено от бота.'));
    disable('power', Boolean(locked || busy || uncertain || !online || data.access !== 'control'));
    disable('refresh', Boolean(locked || busy));
    disable('confirm', Boolean(locked || busy || uncertain));
    disable('cancel', busy);
    const review = Boolean(confirmation && !locked);
    el('widget').classList.toggle('is-confirming', review);
    show('confirm-panel', review);
    show('power', !review);
    show('details', !locked);
    if (review) text('confirm-title', confirmation.action === 'on' ? 'Включить критичное устройство?' : 'Выключить критичное устройство?');
  }
  function theme() {
    document.documentElement.setAttribute('data-theme', api && api.colorScheme === 'dark' ? 'dark' : 'light');
  }
  async function send(action) {
    if (busy || !api || !data || denied) return;
    if (action !== 'read' && (uncertain || data.access !== 'control' || data.state === 'unavailable')) return;
    const revision = data.rev;
    const request = 'w1:' + data.id + ':' + revision + ':' + action;
    if (new TextEncoder().encode(request).length > 64) return;
    busy = true;
    note = action === 'read' ? 'Обновляем состояние…' : 'Передаём команду…';
    render();
    try {
      // Invoke synchronously from the click handler: the SDK requires a gesture.
      const result = await api.request(request);
      const answer = JSON.parse(result.result);
      if (!answer || !accept(answer.state)) throw Error('Invalid answer');
      uncertain = false;
      note = answer.pending ? 'Команда отправлена. Обновите для проверки.' : '';
      if (answer.pending) uncertain = true;
      const c = answer.confirm;
      if (c && data.rev === revision && data.access === 'control' && !denied) {
        if (!/^[a-zA-Z0-9_-]{1,20}$/.test(c.token) || !['on', 'off'].includes(c.action)
            || !Number.isFinite(c.expires) || c.expires <= Date.now()) throw Error('Expired confirmation');
        confirmation = c;
        timer = setTimeout(() => {
          confirmation = null;
          note = 'Подтверждение истекло. Повторите выбор.';
          render();
        }, Math.min(c.expires - Date.now(), 120000));
      } else { confirmation = null; clearTimeout(timer); }
    } catch (error) {
      confirmation = null;
      clearTimeout(timer);
      uncertain = true;
      note = error.code === 'RATE_LIMITED' ? 'Слишком часто. Подождите и обновите.'
        : error.code === 'NOT_ALLOWED' ? 'Действие недоступно. Обновите карточку.'
        : 'Нет ответа. Сначала обновите состояние.';
    }
    busy = false;
    render();
  }
  el('power').addEventListener('click', () => {
    if (!confirmation) send(data && data.state === 'on' ? 'off' : 'on');
  });
  el('refresh').addEventListener('click', () => send('read'));
  el('confirm').addEventListener('click', () => {
    if (confirmation && confirmation.expires > Date.now()) send('confirm:' + confirmation.token);
  });
  el('cancel').addEventListener('click', () => {
    if (busy) return;
    confirmation = null;
    clearTimeout(timer);
    note = 'Действие отменено.';
    render();
  });
  if (api && !denied) {
    api.onEvent('chatDataChanged', read);
    api.onEvent('themeChanged', theme);
    read();
    api.ready();
  }
  theme();
  render();
})();
