import { test } from 'node:test';
import assert from 'node:assert/strict';
import { remoteSessionRows, filterRemoteSessions, type RemoteSession } from './desktopRemoteSessions.ts';
const saved: RemoteSession[] = [
  { id:'live', title:'Live session', agent:'codex', project_path:'/work/project', last_active:20 },
  { id:'saved', title:'Saved session', agent:'claude', project_path:'/work/archive', branch:'fix/images', last_active:10 },
];
test('remote manager includes saved sessions, unindexed terminals and shell tabs without duplicate history', () => {
  const rows = remoteSessionRows(saved, [
    {id:'one', title:'raw terminal title', cwd:'/work/project', sessionId:'live', state:'running'},
    {id:'shell', title:'Shell', cwd:'/work/project', state:'running'},
    {id:'new', title:'New session', cwd:'/work/new', sessionId:'not-indexed', state:'running'},
    {id:'ended', title:'Ended', cwd:'/', state:'exited'},
  ], {live:'idle'}, []);
  assert.equal(rows.length, 4);
  assert.equal(rows.find(r=>r.key==='live')?.tab?.id, 'one');
  assert.equal(rows.find(r=>r.key==='live')?.status, 'Open');
  assert.equal(rows.find(r=>r.key==='saved')?.status, 'History');
});
test('statuses, starred order and search filters use remote roster data', () => {
  const rows = remoteSessionRows(saved, [], {live:'attention'}, ['saved']);
  assert.equal(rows[0].key, 'saved');
  assert.equal(rows.find(r=>r.key==='live')?.status, 'Needs you');
  assert.deepEqual(filterRemoteSessions(rows, 'fix/images','history').map(r=>r.key), ['saved']);
  assert.deepEqual(filterRemoteSessions(rows, 'CODEX','live').map(r=>r.key), ['live']);
  assert.deepEqual(filterRemoteSessions(rows, '/work/archive','starred').map(r=>r.key), ['saved']);
});
