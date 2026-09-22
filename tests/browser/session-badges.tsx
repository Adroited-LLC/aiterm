// Native WebKit/browser fixture: real phase hook, brand icons and production CSS.
import React from 'react';
import { createRoot } from 'react-dom/client';
import { useWorkingSessions } from '../../src/useWorkingSessions';
import AgentIcon from '../../src/components/AgentIcon';
import { agentTint } from '../../src/brand';
import { applySettings, DEFAULT_SETTINGS } from '../../src/settings';
import '../../src/App.css';
applySettings({...DEFAULT_SETTINGS, themeId: new URLSearchParams(location.search).get('theme') || 'carbon'});
let calls = 0;
let renders = 0;
let codexWorking = true;
const agents = ['codex', 'claude', 'grok', 'opencode', 'gemini'];
(window as any).__TAURI_INTERNALS__ = {invoke: async (command: string) => {
  if (command !== 'spine_overview') throw new Error(command);
  calls++;
  return agents.map(agent => ({session_id: agent, phase: agent === 'codex' && !codexWorking ? 'idle' : 'working', last_text: 'Output ' + calls}));
}};
function Badges() {
  const working = useWorkingSessions(true);
  renders++;
  return <div style={{padding: 28, width: 400}}><h3>Static session indicators</h3>{agents.map(agent =>
    <div key={agent} data-session={agent} className={'session-item live' + (working.has(agent) ? ' working' : '')}>
      <div className={'agent-badge' + agentTint(agent).className} data-agent={agent} style={agentTint(agent).style}>
        <AgentIcon agent={agent} mono={working.has(agent)} /><span className="live-dot badge-dot" />
      </div><div className="session-text"><div className="session-title">{agent}</div><div className="session-meta">{working.has(agent) ? 'Working' : 'Open'}</div></div>
    </div>)}</div>;
}
createRoot(document.getElementById('root')!).render(<Badges />);
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
const assert = (ok: boolean, message: string) => { if (!ok) throw new Error(message); };
void (async () => {
  await wait(700);
  assert(!!document.querySelector('[data-session="codex"].working'), 'Codex should show working');
  const before = renders;
  await wait(4500);
  assert(calls >= 3, 'Polls should continue');
  assert(renders <= before + 1, 'Unchanged phases must not repeatedly render on output changes');
  codexWorking = false;
  await wait(2300);
  assert(!document.querySelector('[data-session="codex"].working'), 'Idle Codex must dim');
  assert(!!document.querySelector('[data-session="claude"].working'), 'Claude must stay working');
  codexWorking = true;
  await wait(2300);
  assert(!!document.querySelector('[data-session="codex"].working'), 'Codex must brighten on next turn');
  for (const badge of document.querySelectorAll('.agent-badge')) assert(getComputedStyle(badge).animationName === 'none', 'Badges must stay static');
  document.title = 'PASS: static badges, phase changes, quiet unchanged polls';
})().catch(error => { document.title = 'FAIL: ' + error.message; });
