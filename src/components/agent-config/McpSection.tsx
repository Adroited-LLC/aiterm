import { useEffect, useState } from "react";
import { ClaudeMcpView, claudeMcp } from "../../ipc";

/** MCP servers registered in local files.
 *
 *  The empty case needs words, not a blank list: servers reached as claude.ai
 *  connectors are in no local file, so "none here" is not "no MCP". */
export default function McpSection({ project }: { project: string | null }) {
  const [view, setView] = useState<ClaudeMcpView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    claudeMcp(project).then(setView).catch((e) => setError(String(e)));
  }, [project]);

  if (error) return <div className="acfg-err">{error}</div>;
  if (!view) return <div className="acfg-empty">Reading…</div>;

  return (
    <div>
      <div className="acfg-grp">Registered locally</div>
      {/* A file that is there but unparseable would otherwise be reported as a
          file that could not be read, which is what an absent one says too. */}
      {view.errors.map((e) => (
        <div key={e} className="acfg-err">{e}</div>
      ))}
      {view.servers.length === 0 ? (
        <div className="acfg-empty">
          {view.localConfigRead
            ? "No MCP servers in ~/.claude.json or .mcp.json. Servers connected through claude.ai are not in local files, so a session may still have MCP tools."
            : "No local MCP configuration could be read."}
        </div>
      ) : (
        view.servers.map((s) => (
          <div key={`${s.scope}:${s.name}`} className="acfg-set">
            <span className="acfg-key">{s.name}</span>
            <span className="acfg-val">{s.command ?? "—"}</span>
            <span className="acfg-src">
              {s.scope}
              {s.enabled === false && " · disabled here"}
              {s.enabled === true && " · enabled here"}
            </span>
          </div>
        ))
      )}
    </div>
  );
}
