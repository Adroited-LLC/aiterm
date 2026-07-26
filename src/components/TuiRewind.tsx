import { useEffect, useState } from "react";
import {
  RewindConfirm, RewindPicker, RewindPoint,
  detectRewindConfirm, detectRewindPicker,
} from "../term/screen";
import {
  SETTLE_MS, harvestUpwards, selectByIdentity, selectRow, wait,
} from "../term/drive";

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

/** Identity of a restore point: its prompt plus its diff, which together are
 *  unique in practice even when the same prompt was sent twice. */
const pointKey = (p: RewindPoint) => `${p.prompt}\u0000${p.changes}`;

export default function TuiRewind({ step, write, screen, onDismiss }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The whole list, gathered by paging claude's own. null until gathered. */
  const [allPoints, setAllPoints] = useState<RewindPoint[] | null>(null);
  /** How many gathered so far, so a long list shows progress rather than a
   *  frozen dialog. */
  const [found, setFound] = useState(0);

  // Only the drawn slice of the picker is readable, so page up through it once
  // on open and keep the result. Harmless: nothing commits until Enter.
  useEffect(() => {
    if (step.kind !== "rewind-picker" || allPoints) return;
    let stop = false;
    setBusy(true);
    harvestUpwards(
      () => {
        const p = detectRewindPicker(screen());
        return p && {
          items: p.points,
          highlighted: p.highlighted,
          // claude says how many rows remain above; none means we are there.
          atTop: p.above === 0,
        };
      },
      write,
      pointKey,
      200,
      setFound,
    )
      .then((points) => !stop && setAllPoints(points))
      .catch((e) => !stop && setError(String(e)))
      .finally(() => !stop && setBusy(false));
    return () => { stop = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step.kind]);

  /** Step one: move to a point and continue to the confirmation. */
  const choosePoint = async (point: RewindPoint) => {
    setBusy(true);
    setError(null);
    try {
      const target = pointKey(point);
      await selectByIdentity(
        () => {
          const p = detectRewindPicker(screen());
          return p && { items: p.points, highlighted: p.highlighted };
        },
        write,
        pointKey,
        target,
      );
      const confirmed = detectRewindPicker(screen());
      if (!confirmed || pointKey(confirmed.points[confirmed.highlighted]) !== target) {
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
            {(allPoints ?? step.points).map((p, i) => (
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
                      onClick={() => choosePoint(p)}
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
              ? busy && !allPoints
                ? `Reading the full list from claude… ${found} so far`
                : `${(allPoints ?? step.points).length} points · nothing changes until you confirm`
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
