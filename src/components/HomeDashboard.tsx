/**
 * The centre pane when nothing is open: a home screen, not a blank.
 *
 * What it answers, in the order someone arriving asks: what do I start
 * (the engine picker, centred and large, with the way to add a model beside
 * it); what was I doing (recent sessions, one click back in); is anything
 * waiting on me (sessions blocked on input); where do I work (projects by
 * activity, one click to a shell there). Usage is not here — the strip in
 * the top bar already says it, and a second copy is a second thing to read.
 *
 * Everything here is already in memory — the session list, the alerts, the
 * project list. This draws; it fetches nothing.
 */
import { ReactNode } from "react";
import { ProjectInfo, Session, homeAbbrev, relTime } from "../ipc";
import { Alert } from "./AlertBell";
import AgentIcon from "./AgentIcon";
import Icon from "./Icon";
import { agentTint } from "../brand";
import { Bell, Folder, GitBranch, History, Play, Plus, Rocket } from "lucide-react";

const DAY = 86_400_000;

export default function HomeDashboard({
  sessions, liveIds, alerts, projects, onSelect, onResume, onProject, onGoTab, onAddModel, start,
}: {
  sessions: Session[];
  /** Session ids with a live terminal right now. */
  liveIds: Set<string>;
  alerts: Alert[];
  projects: ProjectInfo[];
  onSelect: (s: Session) => void;
  onResume: (s: Session) => void;
  onProject: (p: ProjectInfo) => void;
  onGoTab: (key: number) => void;
  /** Open Settings → Model access: where a provider and its models are added. */
  onAddModel: () => void;
  /** The start controls — engine, model, effort, the button. */
  start: ReactNode;
}) {
  const now = Date.now();
  const recent = [...sessions].sort((a, b) => b.last_active - a.last_active);
  const month = recent.filter((s) => now - s.last_active < 30 * DAY);

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


  return (
    <div className="home">
      <section className="home-card home-start">
        <div className="home-card-head">
          <Icon of={Rocket} /> <h2>Start a session</h2>
          <button className="act-btn home-add" title="Add a provider or model — opens Model access" onClick={onAddModel}>
            <Icon of={Plus} size="sm" /> Add a model
          </button>
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
        </div>
      </div>
    </div>
  );
}
