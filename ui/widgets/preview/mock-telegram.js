// In-memory simulator. This file is NEVER part of the Telegram widget payload.
(() => {
  const listeners = {};
  const scenario = __SCENARIO__;
  const theme = __THEME__;
  let state = {
    v: 1, id: '42', rev: 1, name: 'Торшер', room: 'Гостиная',
    state: scenario === 'on' ? 'on' : scenario === 'offline' ? 'unavailable' : 'off',
    access: scenario === 'readonly' ? 'view' : scenario === 'denied' ? 'denied' : 'control',
    critical: scenario === 'critical',
  };
  let consent = null;
  let pending = null;
  function announce(kind, message) {
    parent.postMessage({ source: 'ha-widget-demo', kind, message }, '*');
  }
  function reply(extra = {}) {
    api.chatData = JSON.stringify(state);
    return { result: JSON.stringify({ state, ...extra }) };
  }
  const api = {
    colorScheme: theme,
    isForward: scenario === 'forward',
    chatData: JSON.stringify(state),
    onEvent(name, callback) { listeners[name] = callback; },
    ready() { announce('ready', 'Карточка готова. Реальные устройства не подключены.'); },
    async request(command) {
      announce('request', command);
      await new Promise(resolve => setTimeout(resolve, scenario === 'slow' ? 2400 : 650));
      if (scenario === 'error') {
        announce('error', 'Смоделирована потеря ответа. Команда не повторяется.');
        throw Object.assign(new Error('Simulated timeout'), { code: 'TIMEOUT' });
      }
      const [version, id, rev, action, token] = command.split(':');
      if (version !== 'w1' || id !== state.id) throw Error('Invalid target');
      if (scenario === 'revoke') {
        state = { ...state, rev: state.rev + 1, access: 'denied' };
        announce('state', 'Доступ отозван во время ожидания. Управление закрыто.');
        return reply();
      }
      if (action === 'read') {
        state = { ...state, rev: state.rev + 1, state: pending || state.state };
        pending = null;
        announce('state', 'Получено состояние демонстрационного устройства.');
        return reply();
      }
      if (state.access !== 'control' || state.state === 'unavailable') {
        throw Object.assign(new Error('Access denied'), { code: 'NOT_ALLOWED' });
      }
      if (Number(rev) !== state.rev) return reply();
      let target = action;
      if (action === 'confirm') {
        if (!consent || consent.token !== token || consent.expires <= Date.now()) throw Error('Expired');
        target = consent.action;
        consent = null;
      } else if (state.critical) {
        consent = { token: 'demo-consent', action, expires: Date.now() + 30000 };
        announce('state', 'Требуется отдельное подтверждение действия.');
        return reply({ confirm: consent });
      }
      if (!['on', 'off'].includes(target)) throw Error('Invalid action');
      if (scenario === 'pending') {
        pending = target;
        announce('state', 'Команда принята. Фактическое состояние появится после обновления.');
        return reply({ pending: true });
      }
      state = { ...state, rev: state.rev + 1, state: target };
      announce('state', target === 'on' ? 'Демонстрационный свет включён.' : 'Демонстрационный свет выключен.');
      return reply();
    },
  };
  globalThis.TelegramWidget = api;
})();
