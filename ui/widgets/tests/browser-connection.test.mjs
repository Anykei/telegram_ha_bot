import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { access } from 'node:fs/promises';
import { PassThrough, Writable } from 'node:stream';
import test from 'node:test';
import { openBrowser } from './browser-connection.mjs';

function fakeChrome(...behaviors) {
  const attempts = [];
  function launch(_executable, args) {
    const behavior = behaviors[attempts.length];
    assert.ok(behavior, 'Unexpected Chrome launch');
    const profile = args.find(arg => arg.startsWith('--user-data-dir='))
      .slice('--user-data-dir='.length);
    const requests = [];
    const child = new EventEmitter();
    const stderr = new PassThrough();
    const replies = new PassThrough();
    const stop = (code, signal = null) => {
      if (child.exitCode !== null || child.signalCode !== null) return;
      child.exitCode = code;
      child.signalCode = signal;
      queueMicrotask(() => {
        child.emit('exit', code, signal);
        replies.end();
        stderr.end();
        child.emit('close', code, signal);
      });
    };
    const commands = new Writable({
      write(chunk, _encoding, done) {
        const request = JSON.parse(chunk.toString().replace(/\0$/, ''));
        requests.push(request);
        if (request.method === behavior.hangAt) return done();
        if (request.method === behavior.exitAt) {
          stop(1);
          return done();
        }
        const result = {
          'Browser.getVersion': { product: 'Fake Chrome' },
          'Target.createTarget': { targetId: 'tab' },
          'Target.attachToTarget': { sessionId: 'attached' },
          'Runtime.evaluate': { result: { value: 4 } },
        }[request.method] ?? {};
        const answer = request.method === behavior.rejectAt
          ? { error: { code: -32602, message: 'Invalid fixture command' } }
          : { result };
        queueMicrotask(() => {
          replies.write(JSON.stringify({ id: request.id, ...answer }) + '\0');
          if (request.method === 'Browser.close') stop(0);
        });
        done();
      },
    });
    Object.assign(child, {
      stdio: [null, null, stderr, commands, replies],
      stderr,
      pid: behavior.spawnError ? undefined : 1234 + attempts.length,
      exitCode: null,
      signalCode: null,
      kill(signal = 'SIGTERM') {
        stop(null, signal);
        return true;
      },
    });
    attempts.push({ profile, requests, child });
    queueMicrotask(() => {
      if (behavior.stderr) stderr.write(behavior.stderr);
      if (behavior.spawnError) {
        child.emit('error', behavior.spawnError);
        stop(-2);
      } else {
        child.emit('spawn');
      }
    });
    return child;
  }
  return { launch, attempts };
}

const connect = launch => openBrowser('test-chromium', { launch, startupTimeout: 75 });
const methods = ({ requests }) => requests.map(({ method }) => method);

async function assertCleanedUp(attempts) {
  for (const { profile, child } of attempts) {
    assert.ok(child.exitCode !== null || child.signalCode !== null, 'Chrome child stopped');
    await assert.rejects(access(profile), { code: 'ENOENT' });
  }
}

test('retries a stalled Chrome startup with a fresh profile and an attached session', async () => {
  const { launch, attempts } = fakeChrome({ hangAt: 'Target.createTarget' }, {});
  const browser = await connect(launch);
  try {
    assert.equal(attempts.length, 2);
    assert.notEqual(attempts[0].profile, attempts[1].profile);
    await assertCleanedUp(attempts.slice(0, 1));
    await access(attempts[1].profile);
    assert.deepEqual(methods(attempts[0]), ['Browser.getVersion', 'Target.createTarget']);
    assert.deepEqual(methods(attempts[1]), [
      'Browser.getVersion', 'Target.createTarget', 'Target.attachToTarget',
    ]);
    assert.equal(attempts[1].requests[2].params.targetId, 'tab');
    const result = await browser.command('Runtime.evaluate', { expression: '2 + 2' });
    assert.equal(result.result.value, 4);
    assert.equal(attempts[1].requests[3].sessionId, 'attached');
  } finally {
    await browser.close();
  }
  await assertCleanedUp(attempts);
});

test('reports stderr when Chrome disconnects during both startup attempts', async () => {
  const { launch, attempts } = fakeChrome(
    { exitAt: 'Target.createTarget' },
    { exitAt: 'Target.attachToTarget', stderr: 'Chrome could not initialize its renderer\n' },
  );
  await assert.rejects(connect(launch), /Chrome could not initialize its renderer/);
  assert.equal(attempts.length, 2);
  await assertCleanedUp(attempts);
});

test('cleans up without retrying executable or CDP protocol errors', async () => {
  const spawnError = Object.assign(new Error('spawn test-chromium ENOENT'), { code: 'ENOENT' });
  for (const [behavior, expected] of [
    [{ spawnError }, /ENOENT/],
    [{ rejectAt: 'Target.createTarget' }, /Invalid fixture command/],
  ]) {
    const { launch, attempts } = fakeChrome(behavior);
    await assert.rejects(connect(launch), expected);
    assert.equal(attempts.length, 1);
    await assertCleanedUp(attempts);
  }
});
