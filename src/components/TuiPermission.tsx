import { useState } from "react";
import { PermissionRequest, detectPermission } from "../term/screen";
import { SETTLE_MS, selectRow, wait } from "../term/drive";

/**
 * A real dialog in front of claude's tool-permission prompt.
 *
 * Every option claude offers is a button, labelled with claude's own words —
 * including "Yes, allow all edits in tmp/ during this session", which is
 * genuinely useful and which the TUI leaves as an unexplained numbered row.
 *
 * An earlier version offered only Allow/Deny and sent `y`/`n`, on the strength
 * of the CLI's `Confirmation` bindings. Wrong context: this is the generic
 * `Select` widget, where letters do nothing at all. It failed honestly — "sent,
 * but the prompt is still up" — which is the only reason it was a five-minute
 * fix rather than a silent one.
 *
 * The stakes here are higher than for the model picker, so nothing is sent
 * without the screen confirming the highlight first, and the prompt has to
 * actually disappear before we call it done.
 */

interface Props {
  request: PermissionRequest;
  write: (data: string) => void;
  screen: () => string[];
  onDismiss: () => void;
}

/** Colour the obvious refusal differently from the approvals. */
function isRefusal(label: string): boolean {
  const l = label.toLowerCase().trim();
  return l === "no" || l.startsWith("no,");
}

export default function TuiPermission({ request, write, screen, onDismiss }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const answer = async (index: number) => {
    setBusy(true);
    setError(null);
    try {
      await selectRow(() => detectPermission(screen()), write, index);
      for (let i = 0; i < 30; i++) {
        await wait(SETTLE_MS);
        if (!detectPermission(screen())) {
          onDismiss();
          return;
        }
      }
      setError("sent, but the prompt is still up — check the terminal");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="tui-overlay">
      <div className="tui-dialog perm">
        <div className="tui-head">
          <span className="tui-title">{request.title}</span>
          <span className="tui-effort">needs your approval</span>
        </div>

        {request.detail.length > 0 && (
          <div className="perm-detail">
            {request.detail.map((line, i) => (
              <div key={i} className={i === 0 ? "perm-detail-main" : "perm-detail-sub"}>
                {line}
              </div>
            ))}
          </div>
        )}

        <div className="perm-options">
          {request.options.map((o, i) => (
            <button
              key={o.number}
              className={"perm-option" + (isRefusal(o.label) ? " deny" : "")}
              disabled={busy}
              onClick={() => answer(i)}
            >{o.label}</button>
          ))}
        </div>

        {error && <div className="tui-note bad">{error}</div>}

        <div className="tui-foot">
          <span className="tui-hint">Answering here is the same as answering in the terminal.</span>
          <button className="tui-plain" disabled={busy} onClick={onDismiss}>
            Use the terminal
          </button>
        </div>
      </div>
    </div>
  );
}
