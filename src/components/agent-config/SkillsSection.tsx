import { useEffect, useState } from "react";
import { ClaudeSkill, claudeSkills, openPath } from "../../ipc";

/** Skills a session can reach, grouped by the tree they came from. */
export default function SkillsSection({ project }: { project: string | null }) {
  const [skills, setSkills] = useState<ClaudeSkill[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    claudeSkills(project).then(setSkills).catch((e) => setError(String(e)));
  }, [project]);

  if (error) return <div className="acfg-err">{error}</div>;
  if (!skills) return <div className="acfg-empty">Reading…</div>;
  if (skills.length === 0) return <div className="acfg-empty">No skills found.</div>;

  const sources = [...new Set(skills.map((s) => s.source))];
  return (
    <div className="acfg-body">
      {sources.map((src) => (
        <div key={src}>
          <div className="acfg-grp">{src}</div>
          {skills.filter((s) => s.source === src).map((s) => (
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
    </div>
  );
}
