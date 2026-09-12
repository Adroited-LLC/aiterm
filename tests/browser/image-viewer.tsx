// Production component with a local asset URL standing in for Tauri's scoped protocol.
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import ImageView from '../../src/components/ImageView';
import '../../src/App.css';
let fileToken = 0;
let brokenReplacement = false;
const settle = () => new Promise(resolve => setTimeout(resolve, 200));
(window as any).__TAURI_INTERNALS__ = { convertFileSrc: (path: string) => brokenReplacement ? "/tests/browser/images/broken.png" : path, invoke: async () => String(fileToken) };
function Fixture() {
  const [file, setFile] = useState('sample.svg');
  const [active, setActive] = useState(true);
  const [tick, setTick] = useState(0);
  const [results, setResults] = useState<string[]>([]);
  const [running, setRunning] = useState(false);
  const runChecks = async () => {
    setRunning(true); setResults([]); brokenReplacement = false;
    const checks: string[] = [];
    const check = (condition: boolean, text: string) => { if (!condition) throw new Error(text); checks.push("PASS: " + text); };
    try {
      setFile('sample.png'); setActive(true); await settle();
      const img = document.querySelector('.image-canvas') as HTMLImageElement;
      check(!!img?.naturalWidth, 'PNG decoded');
      const source = img.src;
      let changes = 0;
      const observer = new MutationObserver(records => { changes += records.length; });
      observer.observe(img, { attributes: true, attributeFilter: ['src'] });
      for (let i = 0; i < 10; i++) { setTick(v => v + 1); await settle(); }
      observer.disconnect();
      check(document.querySelector('.image-canvas') === img && img.src === source && changes === 0, '10 unrelated project notifications leave image and source untouched');
      fileToken++; setTick(v => v + 1); await settle();
      check(document.querySelector('.image-canvas') === img && img.src !== source && img.complete, 'changed image swaps decoded source without replacing display element');
      const good = img.src;
      brokenReplacement = true; fileToken++; setTick(v => v + 1); await settle();
      check(img.src === good && document.querySelector('.image-canvas') === img, 'damaged replacement retains last good image');
      check(!!document.querySelector('.file-banner[role="alert"]'), 'failed refresh is reported');
      brokenReplacement = false; fileToken++; setTick(v => v + 1); await settle();
      check(img.src !== good && !document.querySelector('.file-banner[role="alert"]'), 'valid replacement recovers without hiding image');
      setActive(false); await settle();
      const hidden = img.src; fileToken++; setTick(v => v + 1); await settle();
      check(img.src === hidden, 'hidden image does not refresh');
      setActive(true); await settle();
      check(img.src !== hidden && document.querySelector('.image-canvas') === img, 'returning tab catches up without replacing image element');
    } catch (error) { checks.push('FAIL: ' + error); }
    finally { brokenReplacement = false; setRunning(false); setResults(checks); }
  };
  return <div style={{ height: '100%', display: 'flex', flexDirection: 'column' }}>
    <div style={{ padding: 8, display: 'flex', flexWrap: 'wrap', gap: 8 }}>
      {['sample.svg', 'sample.png', 'sample.jpeg', 'sample.bmp', 'sample.gif', 'sample.webp', 'sample.ico', 'broken.png'].map(name => <button key={name} onClick={() => setFile(name)}>{name}</button>)}
      <button onClick={() => setActive(v => !v)}>Toggle active</button><button onClick={() => setTick(v => v + 1)}>Unrelated project change</button><button onClick={() => { fileToken++; setTick(v => v + 1); }}>Image changed</button>
      <button disabled={running} onClick={runChecks}>Run refresh checks</button>
    </div>
    {results.length > 0 && <pre>{results.join("\n")}</pre>}
    <div style={{ flex: 1, minHeight: 0, display: active ? 'flex' : 'none' }}><ImageView key={file} path={'/tests/browser/images/' + file} active={active} refreshKey={tick} /></div>
  </div>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
