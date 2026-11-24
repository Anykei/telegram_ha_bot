import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const commandTimeout = 12_000;
const stderrLimit = 8_192;

function connect(executable, profile, launch) {
  const browser = launch(executable, [
    '--headless', '--no-sandbox', '--remote-debugging-pipe', '--no-startup-window',
    '--disable-background-networking', '--disable-component-update', '--disable-sync',
    '--no-first-run', '--no-default-browser-check', '--password-store=basic',
    '--use-mock-keychain', `--user-data-dir=${profile}`,
  ], { stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'] });
  const input = browser.stdio[3];
  const output = browser.stdio[4];
  const closed = new Promise(resolve => browser.once('close', resolve));
  const pending = new Map();
  let nextId = 0;
  let buffer = '';
  let stderr = '';
  let failure;

  function disconnect(error) {
    failure ??= error;
    for (const request of pending.values()) {
      clearTimeout(request.timer);
      request.reject(failure);
    }
    pending.clear();
  }

  function pipeClosed(message, cause) {
    disconnect(Object.assign(new Error(message, { cause }), { code: 'CDP_DISCONNECTED' }));
  }

  browser.once('error', disconnect);
  browser.once('exit', (code, signal) => {
    pipeClosed(`Chrome exited with ${signal ? `signal ${signal}` : `code ${code}`}`);
  });
  browser.stderr.setEncoding('utf8').on('data', chunk => {
    stderr = (stderr + chunk).slice(-stderrLimit);
  });
  input.on('error', error => pipeClosed('Chrome DevTools input failed', error));
  output.on('error', error => pipeClosed('Chrome DevTools output failed', error));
  output.on('end', () => pipeClosed('Chrome closed its DevTools output'));
  output.setEncoding('utf8').on('data', chunk => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf('\0')) !== -1) {
      const frame = buffer.slice(0, end);
      buffer = buffer.slice(end + 1);
      let message;
      try {
        message = JSON.parse(frame);
      } catch (error) {
        disconnect(new Error('Chrome sent invalid DevTools JSON', { cause: error }));
        return;
      }
      const request = pending.get(message.id);
      if (!request) continue;
      clearTimeout(request.timer);
      pending.delete(message.id);
      if (message.error) request.reject(new Error(`${request.method}: ${JSON.stringify(message.error)}`));
      else request.resolve(message.result);
    }
  });

  function command(method, params = {}, sessionId = null, timeout = commandTimeout) {
    if (failure) return Promise.reject(failure);
    return new Promise((resolve, reject) => {
      const id = ++nextId;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(Object.assign(new Error(`Timeout: ${method}`), { code: 'CDP_TIMEOUT' }));
      }, timeout);
      pending.set(id, { method, timer, resolve, reject });
      input.write(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }) + '\0');
    });
  }

  async function stop(gracefully) {
    if (gracefully) await command('Browser.close', {}, null, 1_000).catch(() => {});
    const running = () => browser.pid && browser.exitCode === null && browser.signalCode === null;
    if (running()) browser.kill();
    await Promise.race([closed, delay(1_500, undefined, { ref: false })]);
    if (running()) {
      browser.kill('SIGKILL');
      await Promise.race([closed, delay(1_500, undefined, { ref: false })]);
    }
  }

  function diagnostics() {
    const exit = browser.signalCode ?? browser.exitCode;
    return [`Chrome process status: ${exit ?? 'unknown'}`, stderr.trim() && `Chrome stderr:\n${stderr.trim()}`]
      .filter(Boolean).join('\n');
  }

  return { command, stop, diagnostics };
}

export async function openBrowser(executable, { launch = spawn, startupTimeout = 30_000 } = {}) {
  for (let attempt = 1; ; attempt++) {
    const profile = await mkdtemp(join(tmpdir(), 'ha-widget-browser-'));
    let connection;
    async function close(gracefully = true) {
      try {
        await connection?.stop(gracefully);
      } finally {
        await rm(profile, { recursive: true, force: true, maxRetries: 3 });
      }
    }

    try {
      connection = connect(executable, profile, launch);
      const deadline = performance.now() + startupTimeout;
      const startup = (method, params) =>
        connection.command(method, params, null, Math.max(1, deadline - performance.now()));
      await startup('Browser.getVersion');
      const { targetId } = await startup('Target.createTarget', { url: 'about:blank' });
      const { sessionId } = await startup('Target.attachToTarget', { targetId, flatten: true });
      return { command: (method, params, session = sessionId) => connection.command(method, params, session), close };
    } catch (error) {
      await close(false);
      const details = connection ? `\n${connection.diagnostics()}` : '';
      const message = `Chrome startup attempt ${attempt} failed (${executable}): ${error.message}${details}`;
      if (attempt === 2 || !['CDP_TIMEOUT', 'CDP_DISCONNECTED'].includes(error.code)) {
        throw new Error(message, { cause: error });
      }
      console.warn(`${message}\nRetrying Chrome startup with a new profile.`);
    }
  }
}
