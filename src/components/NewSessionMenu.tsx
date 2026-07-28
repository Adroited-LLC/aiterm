import { useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AgentChoice, agentChoices, homeAbbrev } from "../ipc";

/** What to start, once a directory is chosen. */
export interface StartChoice {
  agentId: string;
  model: string | null;
  effort: string | null;
  mintsSessionId: boolean;
}

/** A directory offered as somewhere to start a session. */
export interface StartPoint {
  path: string;
  /** Directory name, for the row's title line. */
  name: string;
  /** Last activity — a session's, or the directory's own mtime for somewhere
   *  no session has run. Newest first, so the menu opens on where you were. */
  at: number;
}

interface Props {
  /** Where sessions have run before, plus known project directories. */
  places: StartPoint[];
  /** Start a session in this directory, as `choice`. */
  onPick: (path: string, choice: StartChoice) => void;
  onClose: () => void;
}

/**
 * Pick what to start, and where.
 *
 * Two ways in, because the two cases are genuinely different. Almost always
 * you want somewhere you have worked before, and typing a path for that is
 * absurd — so the list is every directory aiterm already knows about, filtered
 * as you type. The first time you work somewhere new there is nothing to pick,
 * so the last row hands off to the OS folder chooser.
 *
 * The list is not restricted to Claude *projects*: `list_projects` only reads
 * `~/Projects`, and a machine where the work lives elsewhere would get an
 * empty menu. Anywhere a session has ever run counts, which is the same set
 * the sidebar is already showing rows for.
 *
 * Above the list sit the agent, model and effort. Only agents actually
 * installed are offered, and the model list comes from whatever that agent
 * publishes — Codex's own cache, Claude's documented aliases. Both model and
 * effort default to blank, meaning "whatever the agent would do on its own",
 * which is the only honest default: any value we picked would be us choosing
 * for you. The agent row is hidden entirely when only one agent is installed,
 * since a picker with one option is furniture.
 */
export default function NewSessionMenu({ places, onPick, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const [agents, setAgents] = useState<AgentChoice[]>([]);
  const [agentId, setAgentId] = useState<string>("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");

  // Installed agents only. The first one is preselected so the menu is usable
  // without touching the row at all.
  useEffect(() => {
    agentChoices()
      .then((list) => {
        setAgents(list);
        setAgentId((cur) => cur || list[0]?.id || "");
      })
      .catch(() => setAgents([]));
  }, []);

  const agent = agents.find((a) => a.id === agentId) ?? null;
  const models = agent?.models ?? [];
  // Effort levels belong to the *model*, not the agent — Codex publishes a
  // different set per model — so the list narrows once one is chosen.
  const efforts = models.find((m) => m.id === model)?.efforts ?? [];

  // Changing agent invalidates both: a Claude alias is not a Codex slug.
  const pickAgent = (id: string) => { setAgentId(id); setModel(""); setEffort(""); };
  // Same for effort when the model changes, unless the new model still offers it.
  const pickModel = (id: string) => {
    setModel(id);
    const next = models.find((m) => m.id === id)?.efforts ?? [];
    setEffort((cur) => (next.includes(cur) ? cur : ""));
  };

  const choice = (): StartChoice => ({
    agentId,
    model: model || null,
    effort: effort || null,
    mintsSessionId: agent?.mints_session_id ?? false,
  });
  const inputRef = useRef<HTMLInputElement>(null);
  const popRef = useRef<HTMLDivElement>(null);

  useEffect(() => inputRef.current?.focus(), []);

  // Click anywhere else closes. Except on the ＋ itself, which owns the
  // toggle: closing here as well would close the menu on pointerdown and let
  // the button's click reopen it a moment later, so the button that opens
  // this menu could never be the button that shuts it.
  useEffect(() => {
    const away = (e: PointerEvent) => {
      const el = e.target as HTMLElement | null;
      if (popRef.current?.contains(el)) return;
      if (el?.closest?.("[data-new-session-trigger]")) return;
      onClose();
    };
    window.addEventListener("pointerdown", away, true);
    return () => window.removeEventListener("pointerdown", away, true);
  }, [onClose]);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q
      ? places.filter(
          (p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q),
        )
      : places;
    // Long enough to be worth scrolling is long enough to be worth trimming:
    // the folder chooser is right there for anything further down.
    return list.slice(0, 12);
  }, [places, query]);

  useEffect(() => setCursor(0), [query]);

  const browse = async () => {
    // Close first: the native chooser is modal, and leaving a popover
    // hovering behind it looks like the app forgot to.
    onClose();
    try {
      const picked = await open({ directory: true, title: "Start a session in…" });
      if (typeof picked === "string") onPick(picked, choice());
    } catch {
      /* the chooser was cancelled or unavailable — nothing to report */
    }
  };

  // `matches.length` is the Browse row: one past the end of the list.
  const choose = (i: number) =>
    i >= matches.length ? browse() : onPick(matches[i].path, choice());

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(c + 1, matches.length));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(c - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(cursor);
    }
  };

  return (
    <div className="new-session-pop" ref={popRef} onKeyDown={onKeyDown}>
      <div className="ns-agents">
        {/* One installed agent is not a choice, so it is not drawn as one. */}
        {agents.length > 1 && (
          <div className="ns-agent-tabs">
            {agents.map((a) => (
              <button
                key={a.id}
                className={"ns-agent-tab" + (a.id === agentId ? " on" : "")}
                onClick={() => pickAgent(a.id)}
              >{a.display_name}</button>
            ))}
          </div>
        )}
        <div className="ns-selects">
          {/* Blank is not a placeholder — it is a real choice, and the right
              default. Letting the agent decide is different from us picking. */}
          <select
            className="ns-select"
            value={model}
            onChange={(e) => pickModel(e.target.value)}
            disabled={models.length === 0}
            title={models.length ? "Model" : "This agent publishes no model list"}
          >
            <option value="">Default model</option>
            {models.map((m) => (
              <option key={m.id} value={m.id}>{m.display_name}</option>
            ))}
          </select>
          <select
            className="ns-select"
            value={effort}
            onChange={(e) => setEffort(e.target.value)}
            disabled={efforts.length === 0}
            title={efforts.length ? "Effort" : "Pick a model first"}
          >
            <option value="">Default effort</option>
            {efforts.map((e) => (
              <option key={e} value={e}>{e}</option>
            ))}
          </select>
        </div>
        {agents.length === 0 && (
          <div className="empty-note">No agent CLI found on this machine.</div>
        )}
      </div>
      <input
        ref={inputRef}
        className="new-session-input"
        placeholder="Start a session in…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      <div className="new-session-list">
        {matches.map((p, i) => (
          <div
            key={p.path}
            className={"new-session-row" + (i === cursor ? " on" : "")}
            onPointerEnter={() => setCursor(i)}
            onClick={() => onPick(p.path, choice())}
          >
            <span className="new-session-name">{p.name}</span>
            <span className="new-session-path">{homeAbbrev(p.path)}</span>
          </div>
        ))}
        {matches.length === 0 && (
          <div className="empty-note">
            {query.trim() ? "No matching directory" : "No directories known yet"}
          </div>
        )}
        <div
          className={"new-session-row browse" + (cursor === matches.length ? " on" : "")}
          onPointerEnter={() => setCursor(matches.length)}
          onClick={browse}
        >
          <span className="new-session-name">Browse for a folder…</span>
        </div>
      </div>
    </div>
  );
}
