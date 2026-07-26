import { useState } from "react";
import {
  RewindConfirm, RewindPicker,
  detectRewindConfirm, detectRewindPicker,
} from "../term/screen";
import { SETTLE_MS, moveHighlight, selectRow, wait } from "../term/drive";

/**
 * A real dialog over `/rewind`, in its two steps: pick a point, then choose
 * what to restore.
 *
 * This is the most destructive thing aiterm drives, so it says the least of
 * its own. claude states the consequences itself — "The conversation will be
 * forked", "The code will be restored +1 -2 in a.txt", and a caveat that files
 * changed by hand or via bash are not covered — and all of it is shown
 * verbatim. Summarising a rollback in our own words is exactly the wrong place
 * to be helpful.
 *
 * Restoring code and conversation independently is the part worth having.
 * Rolling back files but not the conversation leaves claude confidently
 * discussing code that no longer exists; the reverse leaves you with a fresh
 * conversation and stale files. Both, separately, is the only version of this
 * operation that is actually correct — and it is claude's, not ours.
 */

interface Props {
  step: RewindPicker | RewindConfirm;
  write: (data: string) => void;
  screen: () => string[];
  onDismiss: () => void;
}

export default function TuiRewind({ step, write, screen, onDismiss }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /** Step one: move to a point and continue to the confirmation. */
  const choosePoint = async (index: number) => {
    setBusy(true);
    setError(null);
    try {
      await moveHighlight(() => detectRewindPicker(screen()), write, index);
      const confirmed = detectRewindPicker(screen());
      if (!confirmed || confirmed.highlighted !== index) {
        throw new Error("the selection moved unexpectedly — nothing was sent");
      }
      write("\r");
      // The confirmation replaces this screen; the poller will hand it back.
      for (let i = 0; i < 25; i++) {
        await wait(SETTLE_MS);
        if (detectRewindConfirm(screen())) return;
      }
      setError("no confirmation appeared — check the terminal");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  /** Step two: commit one of claude's options. */
  const commit = async (index: number) => {
    setBusy(true);
    setError(null);
    try {
      await selectRow(() => detectRewindConfirm(screen()), write, index);
      for (let i = 0; i < 30; i++) {
        await wait(SETTLE_MS);
        if (!detectRewindConfirm(screen())) {
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

  const cancel = () => {
    write("\x1b");
    onDismiss();
  };

  return (
    <div className="tui-overlay">
      <div className="tui-dialog">
        <div className="tui-head">
          <span className="tui-title">Rewind</span>
          <span className="tui-effort">
            {step.kind === "rewind-picker"
              ? "restore to a point in this conversation"
              : "choose what to restore"}
          </span>
        </div>

        {step.kind === "rewind-picker" ? (
          <div className="tui-list">
            {step.points.map((p, i) => (
              <div key={i} className={"tui-row" + (p.isCurrent ? " current" : "")}>
                <div className="tui-row-text">
                  <span className="tui-row-name">
                    {p.isCurrent ? "Now" : p.prompt}
                    {p.isCurrent && <span className="tui-row-badge">current</span>}
                  </span>
                  {p.changes && <span className="tui-row-desc">{p.changes}</span>}
                </div>
                {!p.isCurrent && (
                  <div className="tui-row-acts">
                    <button
                      className="tui-pick ghost"
                      disabled={busy}
                      onClick={() => choosePoint(i)}
                    >Rewind to before this</button>
                  </div>
                )}
              </div>
            ))}
          </div>
        ) : (
          <>
            <div className="perm-detail">
              <div className="perm-detail-main">{step.prompt}</div>
              {step.when && <div className="perm-detail-sub">{step.when}</div>}
            </div>
            {step.effects.length > 0 && (
              <div className="tui-note">{step.effects.join(" ")}</div>
            )}
            <div className="perm-options">
              {step.options.map((o, i) => (
                <button
                  key={o.number}
                  className="perm-option"
                  disabled={busy}
                  onClick={() => commit(i)}
                >{o.label}</button>
              ))}
            </div>
            {step.warning && <div className="tui-note bad">{step.warning}</div>}
          </>
        )}

        {error && <div className="tui-note bad">{error}</div>}

        <div className="tui-foot">
          <span className="tui-hint">
            {step.kind === "rewind-picker"
              ? "Nothing changes until you confirm on the next step."
              : "This cannot be undone from here."}
          </span>
          <div className="tui-foot-acts">
            <button className="tui-plain" disabled={busy} onClick={onDismiss}>
              Use the terminal
            </button>
            <button className="tui-plain" disabled={busy} onClick={cancel}>
              Cancel
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
