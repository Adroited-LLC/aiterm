import { useState } from "react";
import { Refusal, OpencodeReport } from "../ipc";
import Icon from "./Icon";
import { X } from "lucide-react";

/** "claude-fable-5" → "Fable 5", "claude-opus-4-8" → "Opus 4.8". */
function prettyModel(id: string | null): string {
  if (!id) return "your model";
  const m = id.replace(/^claude-/, "");
  const [name, ...ver] = m.split("-");
  return name.charAt(0).toUpperCase() + name.slice(1) + (ver.length ? ` ${ver.join(".")}` : "");
}

type Kick =
  | { state: "idle" }
  | { state: "running" }
  | { state: "done"; report: OpencodeReport }
  | { state: "error"; message: string };

interface Props {
  refusal: Refusal;
  /** The OpenCode model the kick will run on, for the button label. */
  targetModel: string | null;
  /** Retarget the running session back to its pre-refusal model. */
  onRestore: () => void | Promise<void>;
  /** Dispatch the flagged prompt to OpenCode; resolves with its report. */
  onKick: () => Promise<OpencodeReport>;
  onDismiss: () => void;
}

/**
 * The bar that appears when the classifier downgrades or blocks the active
 * session. It carries the two one-tap moves — restore the model, or hand the
 * flagged prompt to OpenCode — and nothing fires without a click. Both are the
 * human-in-the-loop version of what the user would do by hand; this only makes
 * "by hand" one tap.
 */
export default function RefusalBanner({ refusal, targetModel, onRestore, onKick, onDismiss }: Props) {
  const [kick, setKick] = useState<Kick>({ state: "idle" });
  const restore = prettyModel(refusal.original_model);
  const fallback = prettyModel(refusal.fallback_model);
  const on = targetModel ? ` (${targetModel})` : "";

  const doKick = async () => {
    setKick({ state: "running" });
    try {
      setKick({ state: "done", report: await onKick() });
    } catch (e) {
      setKick({ state: "error", message: String(e) });
    }
  };

  return (
    <div className={"refusal-banner" + (refusal.hard ? " hard" : "")}>
      <div className="refusal-head">
        <span className="refusal-icon">⚠</span>
        <span className="refusal-text">
          {refusal.hard ? (
            <>
              {restore} was <b>blocked</b>
              {refusal.category ? ` (${refusal.category})` : ""} — no fallback.
            </>
          ) : (
            <>
              {restore} was flagged
              {refusal.category ? ` (${refusal.category})` : ""} → switched to {fallback}.
            </>
          )}
        </span>
        <button className="refusal-x" title="Dismiss" onClick={onDismiss}><Icon of={X} size="sm" /></button>
      </div>

      {kick.state === "idle" && (
        <div className="refusal-acts">
          {refusal.original_model && (
            <button className="refusal-btn primary" onClick={() => void onRestore()}>
              Restore {restore}
            </button>
          )}
          <button
            className="refusal-btn"
            onClick={doKick}
            disabled={!refusal.refused_prompt}
            title={refusal.refused_prompt ? "Run the flagged prompt on OpenCode" : "The flagged prompt isn't available to resend"}
          >
            Kick to OpenCode{on}
          </button>
        </div>
      )}
      {kick.state === "running" && (
        <div className="refusal-running">Running on OpenCode{on}…</div>
      )}
      {kick.state === "done" && (
        <div className="refusal-report">
          <div className="refusal-report-head">OpenCode replied · saved as a session in the sidebar</div>
          <pre className="refusal-report-body">{kick.report.text || "(no text returned)"}</pre>
        </div>
      )}
      {kick.state === "error" && <div className="refusal-error">{kick.message}</div>}
    </div>
  );
}
