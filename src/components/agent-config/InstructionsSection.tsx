import { useEffect, useState } from "react";
import { ClaudeDoc, claudeInstructions, homeAbbrev, openPath } from "../../ipc";

/** One row per document, imports nested under the file that pulled them in. */
function DocRow({ doc, depth }: { doc: ClaudeDoc; depth: number }) {
  return (
    <>
      <div className="acfg-file" style={{ paddingLeft: depth * 14 }}>
        <span className="acfg-file-tag">{doc.source}</span>
        <span className={"acfg-file-path" + (doc.present ? "" : " gone")}>
          {homeAbbrev(doc.path)}
        </span>
        {doc.present ? (
          <>
            <span className="acfg-file-state">{doc.lines} lines</span>
            <button className="acfg-open" onClick={() => openPath(doc.path).catch(() => {})}>
              Open
            </button>
          </>
        ) : (
          <span className="acfg-file-state">not present</span>
        )}
      </div>
      {doc.imports.map((d) => (
        <DocRow key={d.path} doc={d} depth={depth + 1} />
      ))}
    </>
  );
}

/** What a session is told before you type anything.
 *
 *  Imports are nested rather than flattened, because "the global file pulls
 *  this in" is different information from "this file is loaded". */
export default function InstructionsSection({ project }: { project: string | null }) {
  const [docs, setDocs] = useState<ClaudeDoc[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    claudeInstructions(project).then(setDocs).catch((e) => setError(String(e)));
  }, [project]);

  if (error) return <div className="acfg-err">{error}</div>;
  if (!docs) return <div className="acfg-empty">Reading…</div>;

  return (
    <div>
      <div className="acfg-grp">Instructions loaded, in order</div>
      {docs.map((d) => <DocRow key={d.path} doc={d} depth={0} />)}
      <div className="acfg-empty">
        Editing these is left to your editor — aiterm writes none of them.
      </div>
    </div>
  );
}
