(() => {
  let screen = __SCREEN__, theme = __THEME__, rev = 1, on = false, mode = 'html';
  const listeners = {};
  const item = (label, detail, action, extra = {}) => ({ label, detail, action, primary: false, selected: false, ...extra });
  function page() {
    const state = on ? 'Включён' : 'Выключен';
    const p = { v: 1, rev, screen, eyebrow: 'Мой дом', title: 'Дом', description: '', hero: null, items: [], nav: [] };
    if (screen === 'home') Object.assign(p, { description: 'Всё нужное — под рукой.', items: [item('Комнаты', '1 комната · 1 устройство', 'rooms'), item('Торшер', state, 'device'), item('Мои настройки', 'Вид интерфейса', 'settings')] });
    if (screen === 'rooms') Object.assign(p, { title: 'Комнаты', description: 'Выберите комнату.', items: [item('Гостиная', '1 устройство', 'room')], nav: [item('Дом', '', 'home')] });
    if (screen === 'room') Object.assign(p, { eyebrow: 'Комнаты', title: 'Гостиная', description: 'Откройте устройство для управления.', items: [item('Торшер', state, 'device')], nav: [item('Комнаты', '', 'rooms'), item('Дом', '', 'home')] });
    if (screen === 'device') Object.assign(p, { eyebrow: 'Гостиная', title: 'Торшер', hero: { label: 'Текущее состояние', value: state }, items: [item(on ? 'Выключить свет' : 'Включить свет', '', on ? 'off' : 'on', {primary: true})], nav: [item('Гостиная', '', 'room'), item('Вид', '', 'settings')] });
    if (screen === 'settings') Object.assign(p, { eyebrow: 'Мои настройки', title: 'Вид интерфейса', description: 'Выбор действует в этом предпросмотре.', items: [item('Обычный', 'Кнопки Telegram', 'classic', {selected: mode === 'classic'}), item('HTML', 'Управление внутри карточки', 'html', {selected: mode === 'html'})], nav: [item('Дом', '', 'home'), item('Торшер', '', 'device')] });
    return p;
  }
  const api = globalThis.TelegramWidget = {
    colorScheme: theme, chatData: JSON.stringify(page()), isForward: false,
    onEvent(name, handler) { listeners[name] = handler; }, ready() {},
    async request(request) {
      test.requests.push(request);
      await new Promise(resolve => setTimeout(resolve, test.delay));
      if (test.fail) throw { code: 'TIMEOUT' };
      if (test.deny) return {result: JSON.stringify({denied: true})};
      const [protocol, revision, action] = request.split(':');
      if (protocol !== 'p1') throw Error('Protocol');
      if (Number(revision) === rev) {
        if (action === 'on' || action === 'off') on = action === 'on';
        if (['home','rooms','room','device','settings'].includes(action)) screen = action;
        if (action === 'classic' || action === 'html') mode = action;
        rev++;
      }
      api.chatData = JSON.stringify(page());
      parent.postMessage({ source: 'ha-panel-demo', mode, page: page(), request }, '*');
      return {result: JSON.stringify({page: page()})};
    }
  };
  const test = globalThis.__panelTest = {
    requests: [], delay: 120, fail: false, deny: false,
    update(extra) { const value = {...page(), ...extra}; api.chatData = JSON.stringify(value); listeners.chatDataChanged(); },
    theme(value) { api.colorScheme = value; listeners.themeChanged(); },
    page,
  };
})();
