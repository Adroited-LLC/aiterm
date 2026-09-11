import test from 'node:test';
import assert from 'node:assert/strict';
import { onTerminalInput } from './terminalUserInput.ts';

test('typing and IME input can acquire control; terminal replies cannot', async () => {
  const { Terminal } = (await import('@xterm/xterm')).default;
  const term = new Terminal({cols:30,rows:10});
  const received: [string, boolean][] = [];
  const sub = onTerminalInput(term, (data, user) => received.push([data, user]));
  try {
    term.input('first', true);
    term.input('日本語', true);
    await new Promise<void>(r => term.write('\x1b[6n', r));
    term.input('\r', true);
    assert.deepEqual(received, [['first',true],['日本語',true],['\x1b[1;1R',false],['\r',true]]);
  } finally {sub.dispose();term.dispose();}
});
