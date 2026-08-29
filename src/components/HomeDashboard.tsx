/**
 * The centre pane when nothing is open: a home screen, not a blank.
 *
 * What it answers, in the order someone arriving asks: what do I start
 * (the engine picker, large); what was I doing (recent sessions, one click
 * back in); is anything waiting on me (sessions blocked on input); how much
 * have I got left (each service's plan and balance); where do I work
 * (projects by activity, one click to a shell there); and how much of this
 * week went where (sessions by engine).
 *
 * Everything here is already in memory — the session list, the usage poller,
 * the alerts, the project list. This draws; it fetches nothing.
 */
import { ReactNode } from "react";
import { ProjectInfo, Session, homeAbbrev, relTime } from "../ipc";
import { UsageSourceAt } from "./UsagePanel";
import { Alert } from "./AlertBell";
import AgentIcon from "./AgentIcon";
import BrandIcon from "./BrandIcon";
import Icon from "./Icon";
import { agentTint, brandForUsageSource } from "../brand";
import { Activity, Bell, Folder, GitBranch, History, Play, Rocket } from "lucide-react";

const DAY = 86_400_000;

/** What the engines are called on a line of prose. */
const ENGINE_NAMES: Record<string, string> = {
  claude: "Claude Code", codex: "Codex", grok: "Grok", opencode: "OpenCode", api: "API", "": "Shell",
};

function greeting(now = new Date()): string {
  const h = now.getHours();
  if (h < 5) return "Up late";
  if (h < 12) return "Good morning";
  if (h < 17) return "Good afternoon";
  if (h < 22) return "Good evening";
  return "Working late";
}

/** "resets in 3h 55m" from an ISO timestamp; "" when unknown or past. */
function resetsIn(iso: string, now = Date.now()): string {
  if (!iso) return "";
  const t = new Date(iso).getTime();
  if (isNaN(t) || t <= now) return "";
  const m = Math.round((t - now) / 60_000);
  if (m < 60) return `resets in ${m}m`;
  const h = Math.floor(m / 60);
  if (h < 48) return `resets in ${h}h ${m % 60}m`;
  return `resets in ${Math.round(h / 24)}d`;
}

/** The one number a service is best summarised by: the fullest plan bar, or
 *  the balance where there is no plan. */
function headline(s: UsageSourceAt): { value: string; sub: string; percent: number | null; severity: string } {
  if (s.state !== "ok" && s.bars.length === 0 && s.amounts.length === 0) {
    return { value: "—", sub: s.detail || s.state, percent: null, severity: "none" };
  }
  if (s.bars.length > 0) {
    const top = s.bars.reduce((a, b) => (b.percent > a.percent ? b : a), s.bars[0]);
    return {
      value: `${Math.round(top.percent)}%`,
      sub: [top.label, resetsIn(top.resets_at)].filter(Boolean).join(" · "),
      percent: Math.min(100, top.percent),
      severity: top.severity,
    };
  }
  const a = s.amounts[0];
  const money = a.currency === "USD";
  const v = money ? `$${a.amount.toFixed(2)}` : `${a.amount}`;
  return {
    value: v,
    sub: a.of !== null ? `${a.label} of ${money ? `$${a.of.toFixed(2)}` : a.of}` : a.label,
    percent: a.of ? Math.min(100, (a.amount / a.of) * 100) : null,
    severity: "none",
  };
}

export default function HomeDashboard({
  sessions, liveIds, usage, alerts, projects, onSelect, onResume, onProject, onGoTab, start,
}: {
  sessions: Session[];
  /** Session ids with a live terminal right now. */
  liveIds: Set<string>;
  usage: UsageSourceAt[];
  alerts: Alert[];
  projects: ProjectInfo[];
  onSelect: (s: Session) => void;
  onResume: (s: Session) => void;
  onProject: (p: ProjectInfo) => void;
  onGoTab: (key: number) => void;
  /** The start controls — engine, model, effort, the button. */
  start: ReactNode;
}) {
  const now = Date.now();
  const recent = [...sessions].sort((a, b) => b.last_active - a.last_active);
  const today = recent.filter((s) => now - s.last_active < DAY);
  const week = recent.filter((s) => now - s.last_active < 7 * DAY);
  const month = recent.filter((s) => now - s.last_active < 30 * DAY);

  // Sessions per engine this week, most first.
  const byEngine = new Map<string, number>();
  for (const s of week) byEngine.set(s.agent, (byEngine.get(s.agent) ?? 0) + 1);
  const engines = [...byEngine.entries()].sort((a, b) => b[1] - a[1]);

  // Projects by how much has happened in them lately.
  const byProject = new Map<string, { count: number; last: number }>();
  for (const s of month) {
    const cur = byProject.get(s.group_path) ?? { count: 0, last: 0 };
    byProject.set(s.group_path, { count: cur.count + 1, last: Math.max(cur.last, s.last_active) });
  }
  const topProjects = [...byProject.entries()]
    .sort((a, b) => b[1].count - a[1].count || b[1].last - a[1].last)
    .slice(0, 6)
    .map(([path, v]) => ({
      path,
      name: projects.find((p) => p.path === path)?.name ?? path.split("/").filter(Boolean).pop() ?? path,
      info: projects.find((p) => p.path === path),
      ...v,
    }));

  const dateLine = new Date().toLocaleDateString(undefined, {
    weekday: "long", month: "long", day: "numeric",
  });

  return (
    <div className="home">
      <header className="home-hero">
        <div className="home-greeting">
          <h1>{greeting()}.</h1>
          <div className="home-date">
            {dateLine}
            <span className="home-dot">·</span>
            {today.length} session{today.length === 1 ? "" : "s"} today
            <span className="home-dot">·</span>
            {week.length} this week
            {liveIds.size > 0 && (
              <>
                <span className="home-dot">·</span>
                <span className="home-live"><span className="live-dot" /> {liveIds.size} live</span>
              </>
            )}
          </div>
        </div>
      </header>

      <section className="home-card home-start">
        <div className="home-card-head">
          <Icon of={Rocket} /> <h2>Start a session</h2>
        </div>
        {start}
      </section>

      <div className="home-grid">
        <section className="home-card home-recent">
          <div className="home-card-head">
            <Icon of={History} /> <h2>Pick up where you left off</h2>
          </div>
          {recent.length === 0 ? (
            <div className="home-empty">No sessions yet — start one above.</div>
          ) : (
            <div className="home-list">
              {recent.slice(0, 8).map((s) => {
                const tint = agentTint(s.agent);
                const live = liveIds.has(s.id);
                return (
                  <div key={s.id} className="home-row" onClick={() => onSelect(s)} title={s.title}>
                    <span className={"home-badge" + tint.className} style={tint.style}>
                      <AgentIcon agent={s.agent} size={15} />
                      {live && <span className="live-dot badge-dot" />}
                    </span>
                    <span className="home-row-text">
                      <span className="home-row-title">{s.title}</span>
                      <span className="home-row-sub">
                        {homeAbbrev(s.project_path)}
                        {s.branch && <span className="home-branch"><Icon of={GitBranch} size="sm" />{s.branch}</span>}
                      </span>
                    </span>
                    <span className="home-row-age">{live ? "live" : relTime(s.last_active)}</span>
                    <button
                      className="home-row-go"
                      title={live ? "Switch to it" : "Resume it"}
                      onClick={(e) => { e.stopPropagation(); live ? onSelect(s) : onResume(s); }}
                    ><Icon of={Play} size="sm" /></button>
                  </div>
                );
              })}
            </div>
          )}
        </section>

        <div className="home-col">
          {alerts.length > 0 && (
            <section className="home-card home-waiting">
              <div className="home-card-head">
                <Icon of={Bell} /> <h2>Waiting for you</h2>
                <span className="home-count">{alerts.length}</span>
              </div>
              <div className="home-list">
                {alerts.slice(0, 5).map((a) => (
                  <div key={a.key} className="home-row" onClick={() => onGoTab(a.key)}>
                    <span className="home-row-text">
                      <span className="home-row-title">{a.title}</span>
                      {a.message && <span className="home-row-sub">{a.message}</span>}
                    </span>
                    <span className="home-row-age">{relTime(a.at)}</span>
                  </div>
                ))}
              </div>
            </section>
          )}

          <section className="home-card home-usage">
            <div className="home-card-head">
              <Icon of={Activity} /> <h2>Usage</h2>
            </div>
            {usage.length === 0 ? (
              <div className="home-empty">Reading plan limits and balances…</div>
            ) : (
              <div className="home-tiles">
                {usage.map((s) => {
                  const h = headline(s);
                  return (
                    <div key={s.id} className={"home-tile sev-" + h.severity + (s.stale ? " stale" : "")}>
                      <div className="home-tile-name">
                        <BrandIcon name={brandForUsageSource(s.id, s.name)} size={14} className="inline" />
                        {s.name}
                      </div>
                      <div className="home-tile-value">{h.value}</div>
                      {h.percent !== null && (
                        <div className="usage-track home-tile-track">
                          <div className={"usage-fill sev-" + h.severity} style={{ width: h.percent + "%" }} />
                        </div>
                      )}
                      <div className="home-tile-sub">{h.sub}</div>
                    </div>
                  );
                })}
              </div>
            )}
          </section>
        </div>

        <section className="home-card home-projects">
          <div className="home-card-head">
            <Icon of={Folder} /> <h2>Projects</h2>
            <span className="home-card-note">last 30 days</span>
          </div>
          {topProjects.length === 0 ? (
            <div className="home-empty">Nothing yet.</div>
          ) : (
            <div className="home-list">
              {topProjects.map((p) => (
                <div
                  key={p.path}
                  className={"home-row" + (p.info ? "" : " quiet")}
                  title={p.info ? `Open a shell in ${homeAbbrev(p.path)}` : homeAbbrev(p.path)}
                  onClick={() => p.info && onProject(p.info)}
                >
                  <span className="home-badge folder"><Icon of={Folder} size="sm" /></span>
                  <span className="home-row-text">
                    <span className="home-row-title">{p.name}</span>
                    <span className="home-row-sub">{homeAbbrev(p.path)}</span>
                  </span>
                  <span className="home-row-age">{p.count} session{p.count === 1 ? "" : "s"} · {relTime(p.last)}</span>
                </div>
              ))}
            </div>
          )}
        </section>

        <section className="home-card home-engines">
          <div className="home-card-head">
            <h2>This week</h2>
            <span className="home-card-note">{week.length} session{week.length === 1 ? "" : "s"}</span>
          </div>
          {engines.length === 0 ? (
            <div className="home-empty">Nothing this week.</div>
          ) : (
            <div className="home-engines-list">
              {engines.map(([agent, n]) => {
                const tint = agentTint(agent);
                const pct = week.length ? (n / week.length) * 100 : 0;
                return (
                  <div key={agent} className="home-engine">
                    <span className={"home-badge" + tint.className} style={tint.style}>
                      <AgentIcon agent={agent} size={15} />
                    </span>
                    <span className="home-engine-name">{ENGINE_NAMES[agent] ?? agent}</span>
                    <span className="home-engine-bar"><span style={{ width: pct + "%", ...(tint.style ?? {}) }} /></span>
                    <span className="home-engine-n">{n}</span>
                  </div>
                );
              })}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
