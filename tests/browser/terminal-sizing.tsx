// Run npm run dev, open /tests/browser/terminal-sizing.html, and run the checks.
// Uses the production TerminalView/xterm with a simulated Tauri/PTY boundary.
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import TerminalView, { type TermTab, type TermHandle } from '../../src/components/TerminalView';
import '../../src/App.css';

type Size = { cols: number; rows: number };
let setDescriptor: React.Dispatch<React.SetStateAction<TermTab>>;
let output: { onmessage: (bytes: ArrayBuffer) => void };
let handle: TermHandle | null = null;
let serial = 0;
let backend: Size = { cols: 47, rows: 11 };
let resizeCalls = 0;
let typed = '';
let canvasAtInput = 0;
let rejectInput = false;
const width = () => document.querySelector('.xterm-screen')!.getBoundingClientRect().width;
const paint = (size: Size) => output.onmessage(new TextEncoder().encode(
  '\x1b[2J\x1b[H' + 'x'.repeat(size.cols) + `\x1b[${size.rows};1HPROMPT > `,
).buffer);
const settle = () => new Promise(resolve => setTimeout(resolve, 220));
(window as any).__TAURI_INTERNALS__ = {
  transformCallback: () => ++serial,
  unregisterCallback: () => {},
  invoke: async (command: string, args: any) => {
    if (command === 'plugin:event|listen') return ++serial;
    if (command === 'tab_attach_desktop') {
      output = args.onOutput;
      return 'fixture-attachment';
    }
    if (command === 'tab_resize') {
      resizeCalls++;
      backend = { cols: args.cols, rows: args.rows };
      paint(backend);
      setDescriptor(tab => ({ ...tab, size: backend }));
    }
    if (command === 'tab_write') {
      if (rejectInput) throw new Error('Simulated stale attachment');
      typed += args.data;
      canvasAtInput = width();
      backend = args.focusSize;
      // Deliberately deliver the resized redraw before the owner event.
      paint(backend);
      setDescriptor(tab => ({ ...tab, focus: 'desktop', size: backend }));
    }
    return null;
  },
};
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
const noop = () => {};
function Fixture() {
  const [tab, setTab] = useState<TermTab>({
    key: 'fixture', title: 'Fixture', cwd: null, command: null,
    slotId: 'fixture', focus: 'desktop', size: backend,
  });
  setDescriptor = setTab;
  const [active, setActive] = useState(true);
  const [results, setResults] = useState<string[]>([]);
  const [running, setRunning] = useState(false);
  const check = (condition: boolean, message: string) => {
    if (!condition) throw new Error(message);
    setResults(previous => [...previous, 'PASS: ' + message]);
  };
  const remote = () => {
    backend = { cols: 47, rows: 11 };
    setTab(t => ({ ...t, focus: 'remote', size: backend }));
  };
  const run = async () => {
    setRunning(true);
    setResults([]);
    typed = "";
    rejectInput = false;
    try {
      await settle();
      const fullWidth = width();
      const fullSize = { ...backend };
      check(fullWidth > 900 && backend.cols > 100 && resizeCalls > 0,
        'Initial attachment synchronizes an already fitted canvas with the PTY');
      const count = resizeCalls;
      remote();
      await settle();
      check(width() < fullWidth / 2 && resizeCalls === count,
        'Viewing a remote owner preserves its grid and does not resize the PTY');
      setTab(t => ({ ...t, focus: 'desktop' }));
      await settle();
      check(width() === fullWidth && backend.cols === fullSize.cols && backend.rows === fullSize.rows,
        'Returning local ownership restores full width and height without user input');
      check(handle!.screen().at(-1)?.includes('PROMPT >') === true,
        'The prompt remains on the visible bottom row');
      setActive(false);
      await settle();
      const beforeHidden = resizeCalls;
      setTab(t => ({ ...t, size: { cols: 47, rows: 11 } }));
      await settle();
      check(resizeCalls === beforeHidden, 'Hidden terminals never publish collapsed dimensions');
      setActive(true);
      await settle();
      check(width() === fullWidth && resizeCalls > beforeHidden,
        'Reactivating the tab synchronizes canvas and PTY');
      remote();
      await settle();
      rejectInput = true;
      await Promise.resolve(handle!.write('rejected')).catch(() => {});
      await settle();
      check(width() < fullWidth / 2 && typed === '', 'Rejected takeover restores the remote viewing grid');
      rejectInput = false;
      handle!.write('first');
      await settle();
      check(typed === 'first' && canvasAtInput === fullWidth && width() === fullWidth,
        'The first typed input fits the canvas before the host redraw and is delivered once');
      check(handle!.screen().at(-1)?.includes('PROMPT >') === true,
        'The prompt stays visible after typed takeover');
    } catch (error) {
      setResults(previous => [...previous, 'FAIL: ' + String(error)]);
    } finally { setRunning(false); }
  };
  return <main style={{ padding: 16 }}>
    <button disabled={running} onClick={run}>Run sizing checks</button>
    <ol>{results.map(result => <li key={result}>{result}</li>)}</ol>
    <div style={{ position: 'relative', width: 1100, height: 480 }}>
      <TerminalView tab={tab} active={active} autoFocus={false}
        fontSize={14} fontFamily="monospace" lineHeight={1} fontWeight={400}
        renderer="dom" theme={{ background: '#000', foreground: '#fff' }}
        onRegister={(_, next) => { handle = next; }} onExit={noop} onActivity={noop}
        onAttention={noop} onNotify={noop} onProgress={noop} onLineSubmit={noop} onOpenFile={noop} />
    </div>
  </main>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
