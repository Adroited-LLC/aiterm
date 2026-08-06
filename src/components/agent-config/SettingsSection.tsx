import { useEffect, useState } from "react";
import { ClaudeSettingsView, claudeSettings, homeAbbrev, openPath } from "../../ipc";

const LAYER_LABEL: Record<string, string> = {
  user: "user",
  project: "project",
  projectLocal: "project local",
  injected: "aiterm",
};

function show(v: unknown): string {
  if (typeof v === "string") return v;
  return JSON.stringify(v);
}

/** The layers, then every setting grouped by concern.
 *
 *  A setting shows the value in force and the file that set it; when more than
 *  one file sets it, the losers are listed too — "project overrides user" is
 *  the sentence this section exists to make sayable. */
export default function SettingsSection({ project }: { project: string | null }) {
  const [view, setView] = useState<ClaudeSettingsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    claudeSettings(project).then(setView).catch((e) => setError(String(e)));
  }, [project]);

  if (error) return <div className="acfg-err">{error}</div>;
  if (!view) return <div className="acfg-empty">Reading…</div>;

  // Groups in the order the backend asked for, then any concern it did not
  // mention. `concern::of` falls everything unrecognised into "Other", which is
  // always in `order` — but this panel's promise is that a setting in effect is
  // always visible, so it does not rely on that holding. A concern that appears
  // out of nowhere gets shown, not dropped.
  const named = view.order.filter((g) => view.settings.some((s) => s.concern === g));
  const extra = [...new Set(view.settings.map((s) => s.concern))].filter(
    (g) => !view.order.includes(g),
  );
  const groups = [...named, ...extra];

  return (
    <div className="acfg-body">
      <div className="acfg-grp">Files</div>
      {view.layers.map((l) => (
        <div key={l.id} className="acfg-file">
          <span className="acfg-file-tag">{LAYER_LABEL[l.id] ?? l.id}</span>
          <span className={"acfg-file-path" + (l.present ? "" : " gone")}>
            {homeAbbrev(l.path)}
          </span>
          {l.present ? (
            <button className="acfg-open" onClick={() => openPath(l.path).catch(() => {})}>
              Open
            </button>
          ) : (
            <span className="acfg-file-state">not present</span>
          )}
          {l.error && <div className="acfg-err">{l.error}</div>}
        </div>
      ))}

      <div className="acfg-grp">Session start</div>
      <div className="acfg-flags">
        {view.injectedFlags.map((f) => (
          <code key={f} className="acfg-flag">{f}</code>
        ))}
        <div className="acfg-empty">
          Added by aiterm to every claude it launches.
          {view.injectedFlags.some((f) => f.includes("skip-permissions")) &&
            " Permission prompts are off in these sessions."}
        </div>
      </div>

      {groups.map((g) => (
        <div key={g}>
          <div className="acfg-grp">{g}</div>
          {view.settings.filter((s) => s.concern === g).map((s) => (
            <div key={s.key} className="acfg-set">
              <span className="acfg-key">{s.key}</span>
              <span className="acfg-val">{show(s.effective)}</span>
              <span className="acfg-src">{LAYER_LABEL[s.winner] ?? s.winner}</span>
              {s.setIn.length > 1 && (
                <div className="acfg-over">
                  also set in{" "}
                  {s.setIn.slice(0, -1).map((x) => LAYER_LABEL[x.layer] ?? x.layer).join(", ")}
                  {" — overridden"}
                </div>
              )}
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
