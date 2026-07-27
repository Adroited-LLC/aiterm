import { useEffect, useRef, useState } from "react";
import {
  ModelConfirm, detectModelConfirm, detectModelPicker, readModelOutcome,
} from "../term/screen";
import { SETTLE_MS, selectRow, wait } from "../term/drive";

/**
 * Step two of `/model`, when claude asks for it: the "Switch model?"
 * confirmation that appears if the switch would invalidate the conversation's
 * cache. The picker dialog sends its ending key, this screen replaces the
 * picker, and the poller swaps this dialog in — same hand-off the two rewind
 * steps use.
 *
 * claude's own words throughout: the cache explanation is shown verbatim and
 * the options are claude's labels, because "Yes, switch to X" names the model
 * and paraphrasing it could name the wrong one.
 *
 * Holds the keyboard like the other dialogs — focus starts on claude's own
 * highlighted row, ↑↓ moves, Enter commits — and the outcome line is read back
 * and shown here, so the answer to "did it actually switch?" is in the dialog
 * that asked, not buried in the terminal. "No, go back" returns the TUI to the
 * picker; this just returns and lets the poller hand the picker dialog back.
 */

interface Props {
  confirm: ModelConfirm;
  write: (data: string) => void;
  screen: () => string[];
  onDismiss: () => void;
}

export default function TuiModelConfirm({ confirm, write, screen, onDismiss }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [focus, setFocus] = useState(confirm.highlighted);
  const optionRefs = useRef<(HTMLButtonElement | null)[]>([]);

  // Take the keyboard while this is up — with focus left in the terminal, an
  // Enter typed now switches the model underneath the dialog.
  useEffect(() => {
    optionRefs.current[focus]?.focus();
  }, [focus]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (busy) return;
    const last = confirm.options.length - 1;
    if (e.key === "ArrowDown" || e.key === "j") {
      e.preventDefault();
      setFocus((f) => (f >= last ? 0 : f + 1));
    } else if (e.key === "ArrowUp" || e.key === "k") {
      e.preventDefault();
      setFocus((f) => (f <= 0 ? last : f - 1));
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancel();
    }
  };

  const answer = async (index: number) => {
    setBusy(true);
    setError(null);
    try {
      await selectRow(() => detectModelConfirm(screen()), write, index);
      for (let i = 0; i < 25; i++) {
        await wait(SETTLE_MS);
        const said = readModelOutcome(screen());
        if (said) {
          setOutcome(said.text);
          await wait(700);
          onDismiss();
          return;
        }
        // "No, go back" re-draws the picker instead of printing an outcome.
        // Return quietly; the poller sees the picker and swaps that dialog in.
        if (!detectModelConfirm(screen()) && detectModelPicker(screen())) return;
      }
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
    <div className="tui-overlay">
      {/* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions */}
      <div className="tui-dialog perm" onKeyDown={onKeyDown}>
        <div className="tui-head">
          <span className="tui-title">Switch model?</span>
        </div>

        {confirm.detail.length > 0 && (
          <div className="perm-detail">
            {confirm.detail.map((line, i) => (
              <div key={i} className={i === 0 ? "perm-detail-main" : "perm-detail-sub"}>
                {line}
              </div>
            ))}
          </div>
        )}

        <div className="perm-options">
          {confirm.options.map((o, i) => (
            <button
              key={o.number}
              ref={(el) => { optionRefs.current[i] = el; }}
              className="perm-option"
              disabled={busy}
              onFocus={() => setFocus(i)}
              onClick={() => answer(i)}
            >{o.label}</button>
          ))}
        </div>

        {outcome && <div className="tui-note ok">{outcome}</div>}
        {error && <div className="tui-note bad">{error}</div>}

        <div className="tui-foot">
          <span className="tui-hint">
            ↑↓ to move · Enter to choose · Esc cancels
          </span>
          <button className="tui-plain" disabled={busy} onClick={onDismiss}>
            Use the terminal
          </button>
        </div>
      </div>
    </div>
  );
}
