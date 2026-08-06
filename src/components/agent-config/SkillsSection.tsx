import { useEffect, useState } from "react";
import { ClaudeSkillsView, claudeSkills, openPath } from "../../ipc";

/** Skills a session can reach, grouped by the tree they came from.
 *
 *  "Can reach" is the whole point: skills of a plugin switched off in settings
 *  are on disk and unusable, so they are counted out loud instead of listed. */
export default function SkillsSection({ project }: { project: string | null }) {
  const [view, setView] = useState<ClaudeSkillsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    claudeSkills(project).then(setView).catch((e) => setError(String(e)));
  }, [project]);

  if (error) return <div className="acfg-err">{error}</div>;
  if (!view) return <div className="acfg-empty">Reading…</div>;

  const disabled = view.disabledPlugins > 0 && (
    <div className="acfg-empty">
      {view.disabledPlugins} installed plugin{view.disabledPlugins === 1 ? " is" : "s are"}{" "}
      disabled; {view.disabledPlugins === 1 ? "its" : "their"} skills are not listed.
    </div>
  );
  const errors = view.errors.map((e) => (
    <div key={e} className="acfg-err">{e}</div>
  ));

  if (view.skills.length === 0) {
    return (
      <div>
        {errors}
        <div className="acfg-empty">No skills found.</div>
        {disabled}
      </div>
    );
  }

  const sources = [...new Set(view.skills.map((s) => s.source))];
  return (
    <div>
      {errors}
      {sources.map((src) => (
        <div key={src}>
          <div className="acfg-grp">{src}</div>
          {view.skills.filter((s) => s.source === src).map((s) => (
            <div key={s.path} className="acfg-set">
              <span className="acfg-key">{s.name}</span>
              <span className="acfg-val acfg-desc">{s.description || "—"}</span>
              <button className="acfg-open" onClick={() => openPath(s.path).catch(() => {})}>
                Open
              </button>
            </div>
          ))}
        </div>
      ))}
      {disabled}
    </div>
  );
}
