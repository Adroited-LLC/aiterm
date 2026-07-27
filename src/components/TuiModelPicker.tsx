import { useEffect, useRef, useState } from "react";
import {
  ModelPicker, detectModelConfirm, detectModelPicker, readModelOutcome,
} from "../term/screen";
import { SETTLE_MS, moveHighlight, wait } from "../term/drive";

/**
 * A real dialog in front of claude's `/model` picker.
 *
 * The TUI underneath is still the thing doing the work — we read what it drew,
 * present it properly, and drive it. Nothing here reimplements model selection;
 * if claude adds a model, it appears, because the list is claude's list.
 *
 * Driving is closed-loop. Every arrow press is followed by re-reading the
 * screen to confirm the highlight actually moved, and the outcome line is read
 * back at the end rather than assumed. The failure mode we refuse to allow is
 * a silent one: sending keys into a screen that has changed under us would pick
 * the wrong model without saying so.
 *
 * Both endings are offered as buttons, because the TUI hides them in a footer
 * as bare keys. "This session" sends `s`; "Make default" sends Enter. Nobody
 * should have to know that.
 *
 * The dialog holds the keyboard while it is up, same as TuiPermission and for
 * the same reason: with focus left in the terminal, keystrokes land on the TUI
 * picker underneath, where Enter is bound to "make default". Rows are menu
 * items — ↑↓ moves between models, ←→ between the two endings, Enter commits
 * the focused one — and every way out hands focus back to the terminal via
 * onDismiss.
 */

interface Props {
  picker: ModelPicker;
  /** Send raw bytes to the terminal. */
  write: (data: string) => void;
  /** Read the terminal's visible screen. */
  screen: () => string[];
  /** Stop showing this dialog for the current picker. */
  onDismiss: () => void;
}

export default function TuiModelPicker({ picker, write, screen, onDismiss }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<string | null>(null);
  /** Which button has the keyboard: [row, 0 = this-session | 1 = default]. */
  const [focus, setFocus] = useState<[number, number]>(() => {
    const current = picker.options.findIndex((o) => o.current);
    return [current >= 0 ? current : 0, 0];
  });
  const buttonRefs = useRef<(HTMLButtonElement | null)[]>([]);

  useEffect(() => {
    buttonRefs.current[focus[0] * 2 + focus[1]]?.focus();
  }, [focus]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (busy) return;
    const last = picker.options.length - 1;
    if (e.key === "ArrowDown" || e.key === "j") {
      e.preventDefault();
      setFocus(([r, c]) => [r >= last ? 0 : r + 1, c]);
    } else if (e.key === "ArrowUp" || e.key === "k") {
      e.preventDefault();
      setFocus(([r, c]) => [r <= 0 ? last : r - 1, c]);
    } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      e.preventDefault();
      setFocus(([r, c]) => [r, c === 0 ? 1 : 0]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancel();
    }
  };

  const commit = async (target: number, key: string) => {
    setBusy(true);
    setError(null);
    try {
      await moveHighlight(() => detectModelPicker(screen()), write, target);
      // Re-read rather than trust the loop's last look: this is the moment
      // just before a keystroke that changes state.
      const confirmed = detectModelPicker(screen());
      if (!confirmed || confirmed.highlighted !== target) {
        throw new Error("the selection moved unexpectedly — nothing was sent");
      }
      write(key);
      for (let i = 0; i < 25; i++) {
        await wait(SETTLE_MS);
        const said = readModelOutcome(screen());
        if (said) {
          setOutcome(said.text);
          await wait(700);
          onDismiss();
          return;
        }
        // A switch that invalidates the cache gets a "Switch model?" screen
        // instead of an outcome. That is the flow continuing, not failing:
        // return quietly and the poller swaps in the confirm dialog. Not
        // onDismiss — that marks the appearance dismissed-to-terminal, which
        // would suppress exactly the dialog we are handing over to.
        if (detectModelConfirm(screen())) return;
      }
      // The keystroke went in but claude never printed a result. Say so rather
      // than claim success.
      setError("sent, but claude did not confirm — check the terminal");
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
    <div className="tui-overlay" onMouseDown={(e) => e.target === e.currentTarget && cancel()}>
      {/* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions */}
      <div className="tui-dialog" onKeyDown={onKeyDown}>
        <div className="tui-head">
          <span className="tui-title">Select model</span>
          {picker.effort && <span className="tui-effort">{picker.effort}</span>}
        </div>

        <div className="tui-list">
          {picker.options.map((o, i) => (
            <div key={o.number} className={"tui-row" + (o.current ? " current" : "")}>
              <div className="tui-row-text">
                <span className="tui-row-name">
                  {o.name}
                  {o.current && <span className="tui-row-badge">running</span>}
                </span>
                <span className="tui-row-desc">{o.description}</span>
              </div>
              <div className="tui-row-acts">
                <button
                  ref={(el) => { buttonRefs.current[i * 2] = el; }}
                  className="tui-pick"
                  disabled={busy}
                  title="Use this model for this session only — your default is untouched"
                  onFocus={() => setFocus([i, 0])}
                  onClick={() => commit(i, "s")}
                >This session</button>
                <button
                  ref={(el) => { buttonRefs.current[i * 2 + 1] = el; }}
                  className="tui-pick ghost"
                  disabled={busy}
                  title="Use this model now and for every new session, in every project"
                  onFocus={() => setFocus([i, 1])}
                  onClick={() => commit(i, "\r")}
                >Make default</button>
              </div>
            </div>
          ))}
        </div>

        {outcome && <div className="tui-note ok">{outcome}</div>}
        {error && <div className="tui-note bad">{error}</div>}

        <div className="tui-foot">
          <span className="tui-hint">
            ↑↓ model · ←→ ending · Enter to choose · “Make default” changes it everywhere
          </span>
          <div className="tui-foot-acts">
            <button className="tui-plain" onClick={onDismiss} disabled={busy}>
              Use the terminal
            </button>
            <button className="tui-plain" onClick={cancel} disabled={busy}>
              Cancel
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
