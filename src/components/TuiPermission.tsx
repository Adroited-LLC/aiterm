import { useState } from "react";
import { PermissionRequest, detectPermission } from "../term/screen";

/**
 * A real dialog in front of claude's tool-permission prompt.
 *
 * This is the most safety-critical screen aiterm draws over, so it is also the
 * most conservative. Only the two answers whose keys are unambiguous in the
 * CLI's own binding table are offered as buttons:
 *
 *   context:"Confirmation" → y: confirm:yes, n: confirm:no, escape: confirm:no
 *
 * Anything else — notably "Yes, and don't ask again for …" — is shown but not
 * clickable, because `enter` is bound to `confirm:yes` rather than "accept the
 * highlighted row", and we have not established what it does when the
 * highlight is elsewhere. Guessing would mean handing out a standing approval
 * nobody asked for. The terminal is right there and it does that job today.
 *
 * The prompt is not dismissable to a "later" state: leaving it half-answered
 * would block the session. Cancelling sends `n`, which is what Escape does in
 * the TUI anyway.
 */

const YES = "y";
const NO = "n";
const SETTLE_MS = 90;

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Recognise the plain yes/no answers among claude's numbered options. */
function classify(label: string): "yes" | "no" | "other" {
  const l = label.toLowerCase().trim();
  if (l === "yes") return "yes";
  if (l === "no" || l.startsWith("no,")) return "no";
  return "other";
}

interface Props {
  request: PermissionRequest;
  write: (data: string) => void;
  screen: () => string[];
  onDismiss: () => void;
}

export default function TuiPermission({ request, write, screen, onDismiss }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const answer = async (key: string) => {
    setBusy(true);
    setError(null);
    try {
      // Re-read immediately before sending: this keystroke approves or denies
      // a real action, so it must not be delivered to a screen that has moved
      // on since the dialog was drawn.
      if (!detectPermission(screen())) {
        throw new Error("the prompt is gone — nothing was sent");
      }
      write(key);
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

  const extra = request.options.filter((o) => classify(o.label) === "other");

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

        {extra.length > 0 && (
          <div className="tui-note">
            Other choices are in the terminal:{" "}
            {extra.map((o) => o.label).join(" · ")}
          </div>
        )}

        {error && <div className="tui-note bad">{error}</div>}

        <div className="tui-foot">
          <button className="tui-plain" disabled={busy} onClick={onDismiss}>
            Use the terminal
          </button>
          <div className="tui-foot-acts">
            <button className="tui-pick ghost" disabled={busy} onClick={() => answer(NO)}>
              Deny
            </button>
            <button className="tui-pick" disabled={busy} onClick={() => answer(YES)}>
              Allow once
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
