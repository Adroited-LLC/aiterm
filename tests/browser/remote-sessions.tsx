// Production manager against an in-memory transport; never touches real sessions or pairings.
import React from 'react';
import { createRoot } from 'react-dom/client';
import DesktopConnections from '../../src/components/DesktopConnections';
import { DEFAULT_SETTINGS, applySettings } from '../../src/settings';
import '../../src/App.css';
const settings = {...DEFAULT_SETTINGS, themeId: new URLSearchParams(location.search).get('theme') || DEFAULT_SETTINGS.themeId};
applySettings(settings);
const state: any = {revision: 1, connection: 'connected', desktop_id: 'fixture', error: null, tabs: [
  {id:'live', sessionId:'open', title:'Remote development', cwd:'/projects/app',state:'running'}
], sessions: [
  {id:'open',title:'Remote development',agent:'codex',project_path:'/projects/app',branch:'main',last_active:6},
  {id:'working',title:'Build the release',agent:'claude',project_path:'/projects/build',branch:'release',last_active:5},
  {id:'waiting',title:'Review a change',agent:'claude',project_path:'/projects/app',last_active:4},
  {id:'history',title:'Saved conversation',agent:'codex',project_path:'/projects/design',branch:'feature/design',last_active:3},
  {id:'old',title:'Earlier research',agent:'codex',project_path:'/projects/research',last_active:2}
], activity:{open:'idle',working:'output',waiting:'attention'}, stars:['history'], agent_caps:{codex:{resume:true,fork:true,delete:true},claude:{resume:true,fork:true,delete:true}},
preview_session:null,preview_messages:[],preview_loading:false,selected_tab:null,screen:null,has_focus:false,attachment_id:null,scrollback:[]};
let notify: (()=>void) | undefined;
const changed = () => {state.revision++; notify?.(); notify=undefined;};
(window as any).__TAURI_INTERNALS__ = {invoke: async (command: string, args: any) => {
  if(command==='desktop_client_list') return [{id:'fixture',name:'Work desktop',address:'fixture.test',fingerprint:'fixture'}];
  if(command==='desktop_client_restore') return;
  if(command==='desktop_client_watch') {if(args.after === state.revision) await new Promise<void>(resolve=>{notify=resolve;}); return structuredClone(state);}
  if(command==='desktop_client_session') {
    const session=state.sessions.find((s:any)=>s.id===args.sessionId);
    if(args.action==='preview') {state.preview_session=args.sessionId;state.preview_messages=[{role:'user',text:'Can we continue this saved conversation?'},{role:'assistant',text:'Yes. The conversation remains on the remote desktop. Resume it whenever you are ready.'}];}
    if(args.action==='rename') session.custom_title=args.title;
    if(args.action==='star') state.stars=args.on?[...state.stars,args.sessionId]:state.stars.filter((s:string)=>s!==args.sessionId);
    if(args.action==='delete') {state.sessions=state.sessions.filter((s:any)=>s.id!==args.sessionId);state.preview_session=null;}
    if(args.action==='open') {state.preview_session=null;state.selected_tab='resumed';state.tabs.push({id:'resumed',sessionId:args.sessionId,cwd:session.project_path,title:session.title,state:'running'});}
    changed(); return;
  }
  if(command==='desktop_client_attach') {state.selected_tab=args.tabId;state.preview_session=null;changed();return;}
  if(command.startsWith('desktop_client_')) return;
  throw Error('Unexpected fixture command: '+command);
}};
createRoot(document.getElementById('root')!).render(<DesktopConnections settings={settings} onClose={()=>{}} />);
