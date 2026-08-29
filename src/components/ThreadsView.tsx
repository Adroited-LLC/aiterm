/**
 * The Threads tab: the session list as the librarian sees it — bodies of
 * work, each with where it left off and what would come next, one click
 * back in.
 *
 * Drawn entirely from the librarian's store and the session list; it fetches
 * nothing. A thread's facts are computed here: its sessions, which projects
 * and engines they span, the most recent one (whose summary is "where it
 * left off"), and when that was.
 */
import { useEffect, useMemo, useState } from "react";
import { LibStore, Session, homeAbbrev } from "../ipc";
import { LibrarianCtl } from "../librarian";
import { fmtTimeShort, fullTime, useTimeFormat } from "../timefmt";
import AgentIcon from "./AgentIcon";
import Icon from "./Icon";
import { agentTint } from "../brand";
import { BookOpen, ChevronRight, Loader2, Pencil, Play, Plus, Sparkles, Square, Tag, Wand2, X } from "lucide-react";

/** The tag chips of a thread or a session: the model's, then the person's
 *  (outlined, with a ×), then a + that opens a one-line input. Clicking a
 *  chip filters the view by it. */
function Tags({ tags, userTags, onAdd, onRemove, onPick, active }: {
  tags: string[];
  userTags: string[];
  onAdd: (t: string) => void;
  onRemove: (t: string) => void;
  onPick: (t: string) => void;
  active: string | null;
}) {
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");
  const commit = () => {
    const t = draft.trim();
    if (t) onAdd(t);
    setDraft("");
    setAdding(false);
  };
  return (
    <div className="thr-tags" onClick={(e) => e.stopPropagation()}>
      {tags.filter((t) => !userTags.includes(t)).map((g) => (
        <button key={g} className={"thr-tag" + (active === g ? " on" : "")} onClick={() => onPick(g)} title="Show only this tag">{g}</button>
      ))}
      {userTags.map((g) => (
        <span key={"u:" + g} className={"thr-tag user" + (active === g ? " on" : "")} title="Your tag — the librarian keeps it and reads it as fact">
          <button className="thr-tag-text" onClick={() => onPick(g)}>{g}</button>
          <button className="thr-tag-x" title="Remove" onClick={() => onRemove(g)}><Icon of={X} size="sm" /></button>
        </span>
      ))}
      {adding ? (
        <input
          className="thr-tag-input"
          value={draft}
          autoFocus
          placeholder="tag"
          spellCheck={false}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") { setDraft(""); setAdding(false); }
          }}
          onBlur={commit}
        />
      ) : (
        <button className="thr-tag add" title="Add a tag of your own" onClick={() => setAdding(true)}><Icon of={Plus} size="sm" /></button>
      )}
    </div>
  );
}

interface ThreadCard {
  id: string;
  name: string;
  description: string;
  tags: string[];
  sessions: Session[];
  /** Most recent session — its entry says where the thread left off. */
  latest: Session;
  projects: string[];
  engines: string[];
  last: number;
}

/** A stable hue per thread, so a thread keeps its colour across runs. */
function hueOf(id: string): number {
  let h = 0;
  for (const c of id) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}

export function buildThreads(store: LibStore, sessions: Session[]): { threads: ThreadCard[]; loose: Session[]; uncatalogued: number } {
  const byThread = new Map<string, Session[]>();
  const loose: Session[] = [];
  let uncatalogued = 0;
  for (const s of sessions) {
    const e = store.sessions[s.id];
    if (!e) { uncatalogued++; continue; }
    if (e.thread && store.threads[e.thread]) {
      (byThread.get(e.thread) ?? byThread.set(e.thread, []).get(e.thread)!).push(s);
    } else {
      loose.push(s);
    }
  }
  const threads: ThreadCard[] = [...byThread.entries()].map(([id, ss]) => {
    ss.sort((a, b) => b.last_active - a.last_active);
    const t = store.threads[id];
    return {
      id,
      name: t.name,
      description: t.description,
      tags: t.tags,
      sessions: ss,
      latest: ss[0],
      projects: [...new Set(ss.map((s) => s.project_path.split("/").filter(Boolean).pop() ?? s.project_path))],
      engines: [...new Set(ss.map((s) => s.agent))],
      last: ss[0].last_active,
    };
  });
  threads.sort((a, b) => b.last - a.last);
  loose.sort((a, b) => b.last_active - a.last_active);
  return { threads, loose, uncatalogued };
}

export default function ThreadsView({
  lib, sessions, liveIds, onSelect, onResume, onOpenSettings, canResume,
}: {
  lib: LibrarianCtl;
  sessions: Session[];
  liveIds: Set<string>;
  onSelect: (s: Session) => void;
  onResume: (s: Session) => void;
  onOpenSettings: () => void;
  canResume: (s: Session) => boolean;
}) {
  const { format: timeFormat } = useTimeFormat();
  const when = (ms: number) => fmtTimeShort(ms, timeFormat);
  const { threads, loose, uncatalogued } = useMemo(() => buildThreads(lib.store, sessions), [lib.store, sessions]);
  // Opening the tab re-reads the store: a run started elsewhere — or a test
  // from a shell — may have written since it was last loaded.
  useEffect(() => { void lib.reload(); }, []);
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  /** A tag to show only — set by clicking a chip. */
  const [tagFilter, setTagFilter] = useState<string | null>(null);
  const [tagging, setTagging] = useState<string | null>(null);
  const allTagsOf = (t: { tags: string[]; user_tags?: string[] }) => [...t.tags, ...(t.user_tags ?? [])];
  const pickTag = (t: string) => setTagFilter((cur) => (cur === t ? null : t));
  const toggle = (id: string) => setOpen((prev) => {
    const next = new Set(prev);
    next.has(id) ? next.delete(id) : next.add(id);
    return next;
  });

  const jumpIn = (s: Session) => (liveIds.has(s.id) ? onSelect(s) : canResume(s) ? onResume(s) : onSelect(s));

  const row = (s: Session) => {
    const e = lib.store.sessions[s.id];
    const live = liveIds.has(s.id);
    const tint = agentTint(s.agent);
    const showTags = tagging === s.id || (e?.user_tags.length ?? 0) > 0;
    return (
      <div key={s.id} className="thr-row-wrap">
        <div className="thr-row" onClick={() => onSelect(s)} title={s.title}>
          <span className={"thr-badge" + tint.className} style={tint.style}>
            <AgentIcon agent={s.agent} size={12} />
            {live && <span className="live-dot badge-dot" />}
          </span>
          <span className="thr-row-text">
            <span className="thr-row-name">{e?.name ?? s.title}</span>
            {e?.summary && <span className="thr-row-sub">{e.summary}</span>}
          </span>
          <span className="thr-row-age" title={fullTime(s.last_active)}>{live ? "live" : when(s.last_active)}</span>
          {e && (
            <button
              className={"thr-row-go" + (showTags ? " stay" : "")}
              title="Tag this session"
              onClick={(ev) => { ev.stopPropagation(); setTagging((cur) => (cur === s.id ? null : s.id)); }}
            ><Icon of={Tag} size="sm" /></button>
          )}
          <button
            className="thr-row-go"
            title={live ? "Switch to it" : "Pick it up"}
            onClick={(ev) => { ev.stopPropagation(); jumpIn(s); }}
          ><Icon of={Play} size="sm" /></button>
        </div>
        {e && showTags && (
          <div className="thr-row-tags">
            <Tags
              tags={tagging === s.id ? e.tags : []}
              userTags={e.user_tags}
              active={tagFilter}
              onPick={pickTag}
              onAdd={(t) => void lib.tag({ kind: "session", id: s.id }, t, true)}
              onRemove={(t) => void lib.tag({ kind: "session", id: s.id }, t, false)}
            />
          </div>
        )}
      </div>
    );
  };

  if (!lib.ready) {
    return (
      <div className="thr-empty">
        <Icon of={BookOpen} size="lg" />
        <div className="thr-empty-title">Threads need a librarian</div>
        <div className="thr-empty-text">
          A small model reads each session once, names it, tags it and files it
          under the body of work it belongs to. This tab is what it writes.
        </div>
        <button className="tui-pick" onClick={onOpenSettings}>Choose a model…</button>
      </div>
    );
  }

  return (
    <div className="thr">
      <div className="thr-head">
        <span className="thr-stat">
          {threads.length} thread{threads.length === 1 ? "" : "s"}
          {uncatalogued > 0 && <> · {uncatalogued} not yet read</>}
        </span>
        {lib.running ? (
          <span className="thr-running">
            <Icon of={Loader2} size="sm" className="spin" />
            {lib.progress ? `${lib.progress.done} of ${lib.progress.total}` : "reading…"}
            <span className="thr-hint">· ~2 min per 8</span>
            <button className="thr-stop" title="Stop after this batch" onClick={lib.stop}><Icon of={Square} size="sm" /></button>
          </span>
        ) : lib.tidying ? (
          <span className="thr-running"><Icon of={Loader2} size="sm" className="spin" /> tidying up…</span>
        ) : lib.pending.length > 0 ? (
          <button className="thr-run" onClick={() => void lib.run()} title="Read the sessions the librarian has not seen yet">
            <Icon of={Sparkles} size="sm" /> Catalogue {lib.pending.length}
          </button>
        ) : lib.tidyDue ? (
          <button className="thr-run" onClick={() => void lib.tidy()} title="One look at everything: merge threads that are the same work, file loose sessions">
            <Icon of={Wand2} size="sm" /> Tidy up
          </button>
        ) : threads.length > 1 ? (
          <button className="thr-run quiet" onClick={() => void lib.tidy()} title="One look at everything: merge threads that are the same work, file loose sessions">
            <Icon of={Wand2} size="sm" />
          </button>
        ) : null}
      </div>
      {lib.running && lib.progress && (
        <div className="thr-bar"><span style={{ width: `${Math.max(3, (100 * lib.progress.done) / Math.max(1, lib.progress.total))}%` }} /></div>
      )}
      {lib.report?.errors.length ? (
        <div className="thr-error">{lib.report.errors[0]}</div>
      ) : null}
      {lib.tidyReport && "error" in lib.tidyReport && (
        <div className="thr-error">{lib.tidyReport.error}</div>
      )}

      {threads.length === 0 && loose.length === 0 && (
        <div className="thr-empty-text">
          {lib.running ? "Reading the first batch — about eight sessions at a time; the first threads land in a minute or two." : "Nothing catalogued yet — press Catalogue above."}
        </div>
      )}

      {tagFilter && (
        <div className="thr-filter">
          <Icon of={Tag} size="sm" /> {tagFilter}
          <button className="thr-tag-x" title="Show all" onClick={() => setTagFilter(null)}><Icon of={X} size="sm" /></button>
        </div>
      )}
      {threads.filter((t) => !tagFilter
        || allTagsOf({ tags: t.tags, user_tags: lib.store.threads[t.id]?.user_tags }).includes(tagFilter)
        || t.sessions.some((s) => lib.store.sessions[s.id]?.user_tags.includes(tagFilter))
      ).map((t) => {
        const latest = lib.store.sessions[t.latest.id];
        const isOpen = open.has(t.id);
        const hue = hueOf(t.id);
        return (
          <section
            key={t.id}
            className={"thr-card" + (isOpen ? " open" : "")}
            style={{ ["--thr" as string]: `hsl(${hue} 55% 58%)` }}
          >
            <div className="thr-card-head" onClick={() => toggle(t.id)}>
              <span className={"thr-chev" + (isOpen ? " open" : "")}><Icon of={ChevronRight} size="sm" /></span>
              {editing === t.id ? (
                <input
                  className="thr-rename"
                  value={draft}
                  autoFocus
                  onClick={(e) => e.stopPropagation()}
                  onChange={(e) => setDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") { void lib.renameThread(t.id, draft); setEditing(null); }
                    if (e.key === "Escape") setEditing(null);
                  }}
                  onBlur={() => setEditing(null)}
                />
              ) : (
                <span className="thr-name">{t.name}</span>
              )}
              <button
                className="thr-edit"
                title="Rename"
                onClick={(e) => { e.stopPropagation(); setDraft(t.name); setEditing(t.id); }}
              ><Icon of={Pencil} size="sm" /></button>
              <span className="thr-engines">
                {t.engines.map((a) => <AgentIcon key={a} agent={a} size={12} />)}
              </span>
              <span className="thr-age" title={fullTime(t.last)}>{when(t.last)}</span>
            </div>
            <div className="thr-meta">
              <span>{t.sessions.length} session{t.sessions.length === 1 ? "" : "s"}</span>
              <span className="thr-dot">·</span>
              <span className="thr-projects" title={t.sessions.map((s) => homeAbbrev(s.project_path)).join("\n")}>
                {t.projects.slice(0, 3).join(", ")}{t.projects.length > 3 ? ` +${t.projects.length - 3}` : ""}
              </span>
            </div>
            <Tags
              tags={t.tags}
              userTags={lib.store.threads[t.id]?.user_tags ?? []}
              active={tagFilter}
              onPick={pickTag}
              onAdd={(g) => void lib.tag({ kind: "thread", id: t.id }, g, true)}
              onRemove={(g) => void lib.tag({ kind: "thread", id: t.id }, g, false)}
            />
            {latest?.summary && (
              <div className="thr-leftoff">
                <span className="thr-k">Left off</span> {latest.summary}
              </div>
            )}
            {latest?.next && (
              <div className="thr-next">
                <span className="thr-k">Next</span> {latest.next}
              </div>
            )}
            <div className="thr-acts">
              <button className="tui-pick thr-pickup" onClick={() => jumpIn(t.latest)} title={t.latest.title}>
                <Icon of={Play} size="sm" /> {liveIds.has(t.latest.id) ? "Switch to it" : "Pick up"}
              </button>
              <button className="tui-plain thr-more" onClick={() => toggle(t.id)}>
                {isOpen ? "Hide sessions" : `All ${t.sessions.length}`}
              </button>
            </div>
            {isOpen && <div className="thr-list">{t.sessions.map(row)}</div>}
          </section>
        );
      })}

      {loose.length > 0 && (
        <section className="thr-loose">
          <div className="thr-loose-head">Loose ends <span className="thr-count">{loose.length}</span></div>
          <div className="thr-list">{loose.filter((s) => !tagFilter || allTagsOf(lib.store.sessions[s.id]).includes(tagFilter)).map(row)}</div>
        </section>
      )}
    </div>
  );
}
