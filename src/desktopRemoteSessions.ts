export type RemoteSession = {
  id: string; agent: string; title: string; custom_title?: string;
  project_path: string; group_path?: string; branch?: string; last_active: number;
};
export type RemoteTab = { id: string; title: string; cwd: string; sessionId?: string; state: string };
export type SessionRow = { key: string; session?: RemoteSession; tab?: RemoteTab; title: string; path: string; status: string; starred: boolean };

/** Include shell tabs and unindexed live sessions without duplicating saved history. */
export function remoteSessionRows(sessions: RemoteSession[], tabs: RemoteTab[], activity: Record<string, string>, stars: string[]): SessionRow[] {
  const live = tabs.filter(t => t.state === 'running');
  const indexed = new Set(sessions.map(s => s.id));
  const rows: SessionRow[] = sessions.map(session => {
    const tab = live.find(t => t.sessionId === session.id);
    const phase = activity[session.id];
    return { key: session.id, session, tab, title: session.custom_title || session.title || 'Untitled session',
      path: session.group_path || session.project_path || '', starred: stars.includes(session.id),
      status: phase === 'attention' ? 'Needs you' : phase === 'output' ? 'Working' : tab || phase !== undefined ? 'Open' : 'History' };
  });
  for (const tab of live) if (!tab.sessionId || !indexed.has(tab.sessionId)) {
    rows.push({key: `tab:${tab.id}`, tab, title: tab.title || 'Terminal', path: tab.cwd, status: 'Open', starred: false});
  }
  return rows.sort((a,b) => Number(b.starred) - Number(a.starred) || Number(b.status !== 'History') - Number(a.status !== 'History') ||
    (b.session?.last_active ?? 0) - (a.session?.last_active ?? 0) || a.key.localeCompare(b.key));
}
export function filterRemoteSessions(rows: SessionRow[], query: string, filter: string): SessionRow[] {
  const text = query.trim().toLocaleLowerCase();
  return rows.filter(row => (filter !== 'live' || row.status !== 'History') && (filter !== 'history' || row.status === 'History') &&
    (filter !== 'starred' || row.starred) && (!text || `${row.title} ${row.path} ${row.session?.agent ?? ''} ${row.session?.branch ?? ''}`.toLocaleLowerCase().includes(text)));
}
