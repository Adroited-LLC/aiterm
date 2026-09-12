// Production component with a local asset URL standing in for Tauri's scoped protocol.
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import ImageView from '../../src/components/ImageView';
import '../../src/App.css';
(window as any).__TAURI_INTERNALS__ = { convertFileSrc: (path: string) => path, invoke: async () => null };
function Fixture() {
  const [file, setFile] = useState('sample.svg');
  const [active, setActive] = useState(true);
  const [tick, setTick] = useState(0);
  return <div style={{ height: '100%', display: 'flex', flexDirection: 'column' }}>
    <div style={{ padding: 8, display: 'flex', flexWrap: 'wrap', gap: 8 }}>
      {['sample.svg', 'sample.png', 'sample.jpeg', 'sample.bmp', 'sample.gif', 'sample.webp', 'sample.ico', 'broken.png'].map(name => <button key={name} onClick={() => setFile(name)}>{name}</button>)}
      <button onClick={() => setActive(v => !v)}>Toggle active</button><button onClick={() => setTick(v => v + 1)}>File changed</button>
    </div>
    <div style={{ flex: 1, minHeight: 0, display: active ? 'flex' : 'none' }}><ImageView key={file} path={'/tests/browser/images/' + file} active={active} refreshKey={tick} /></div>
  </div>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
