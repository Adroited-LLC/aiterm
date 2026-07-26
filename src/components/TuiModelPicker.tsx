import { useState } from "react";
import { ModelPicker, detectModelPicker, readModelOutcome } from "../term/screen";

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
 */

const KEY_DOWN = "\x1b[B";
const KEY_UP = "\x1b[A";
/** Long enough for claude to repaint between presses on a loaded machine. */
const SETTLE_MS = 90;
/** More than any real list needs; stops a mis-detection spinning forever. */
const MAX_STEPS = 24;

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

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

  /** Walk the highlight to `target`, checking the screen after every press. */
  const moveTo = async (target: number) => {
    for (let step = 0; step < MAX_STEPS; step++) {
      const now = detectModelPicker(screen());
      if (!now) throw new Error("the picker closed before the choice landed");
      if (now.highlighted === target) return;
      write(now.highlighted < target ? KEY_DOWN : KEY_UP);
      await wait(SETTLE_MS);
    }
    throw new Error("could not move the selection to that model");
  };

  const commit = async (target: number, key: string) => {
    setBusy(true);
    setError(null);
    try {
      await moveTo(target);
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
      <div className="tui-dialog">
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
                  className="tui-pick"
                  disabled={busy}
                  title="Use this model for this session only — your default is untouched"
                  onClick={() => commit(i, "s")}
                >This session</button>
                <button
                  className="tui-pick ghost"
                  disabled={busy}
                  title="Use this model now and for every new session, in every project"
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
            “This session” leaves your default alone. “Make default” changes it everywhere.
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
