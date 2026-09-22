import React, { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import BrandIcon from '../../src/components/BrandIcon';
import { useSpineOverview } from '../../src/useSpineOverview';
import '../../src/App.css';
let calls = 0, overviewRenders = 0;
let text = 'Unchanged';
let setBrand: (name: string) => void;
(window as any).__TAURI_INTERNALS__ = {
  transformCallback: () => 1,
  invoke: async (command: string) => {
    if (command === 'plugin:event|listen') return 1;
    if (command === 'plugin:event|unlisten') return;
    if (command !== 'spine_overview') throw new Error(command);
    calls++;
    return [{session_id:'s',agent:'codex',phase:'idle',detail:'',turn_open:false,turn_started_ts:null,last_text:text,last_tool:null}];
  },
};
function Overview() {
  const rows = useSpineOverview(true);
  overviewRenders++;
  return <div id="summary">{rows.get('s')?.last_text}</div>;
}
function Icons() {
  const [tick, setTick] = useState(0);
  const [name, updateBrand] = useState('claude');
  setBrand = updateBrand;
  useEffect(() => {
    const id = setInterval(() => setTick(t => t + 1), 70);
    return () => clearInterval(id);
  }, []);
  return <div id="icons" data-tick={tick}><BrandIcon name={name} variant="mono" /></div>;
}
createRoot(document.getElementById('root')!).render(<><Icons /><Overview /></>);
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
const assert = (ok: boolean, label: string) => { if (!ok) throw Error(label); };
void (async () => {
  await wait(400);
  const original = document.querySelector('#icons svg');
  assert(!!original, 'Initial icon missing');
  await wait(700);
  assert(document.querySelector('#icons svg') === original, 'Unchanged icon was rebuilt during parent updates');
  setBrand('openai');
  await wait(250);
  assert(!!document.querySelector('#icons .openai svg'), 'A changed brand must update its SVG');
  const before = overviewRenders;
  await wait(4300);
  assert(calls >= 3, 'Overview did not poll');
  assert(overviewRenders <= before + 1, 'Identical overview polls keep rerendering the dashboard');
  text = 'New content';
  await wait(2200);
  assert(document.querySelector('#summary')?.textContent === text, 'Real content change did not render');
  document.title = 'PASS: stable icons, quiet overview, real updates preserved';
})().catch(error => { document.title = 'FAIL: ' + error.message; });
