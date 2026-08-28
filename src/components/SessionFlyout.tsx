/**
 * The card that opens beside a sidebar row on hover: everything `detail.rs`
 * could read from the transcript, laid out to jog a memory.
 *
 * Ordered by what makes recognition happen fastest — the opening ask and the
 * last exchange first, because "what was this about" is the question; then
 * when and how long; then the machinery (model, mode, context used, tools,
 * files) for the cases where the words alone do not place it.
 *
 * Positioned by the caller (fixed, beside the row); this only draws.
 */
import { Session, SessionDetail, homeAbbrev } from "../ipc";
import AgentIcon from "./AgentIcon";
import BrandIcon from "./BrandIcon";
import Icon from "./Icon";
import { brandForModel } from "../brand";
import { useState } from "react";
import { Check, Clock, Copy, FileCode2, GitBranch, GitPullRequest, Hammer, MessageSquare, PieChart, Folder } from "lucide-react";

/** "1h 22m", "14m", "40s". */
export function fmtDuration(ms: number): string {
  if (ms < 0) return "";
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const d = Math.floor(h / 24);
  if (d >= 1) return `${d}d ${h % 24}h`;
  return `${h}h ${m % 60}m`;
}

/** "Aug 28, 7:11 PM" — the day matters, the seconds do not. */
export function fmtWhen(iso: string): string {
  const t = new Date(iso);
  if (isNaN(t.getTime())) return iso;
  return t.toLocaleString(undefined, {
    month: "short", day: "numeric", hour: "numeric", minute: "2-digit",
  });
}

export function fmtTok(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return `${n}`;
}

/** Claude's context window by model, when the transcript did not say. */
function windowFor(models: string[], said: number | null): number | null {
  if (said) return said;
  if (models.length === 0) return null;
  return 200_000;
}

/** The card as plain text, for the clipboard: the same facts in the same
 *  order, one per line, so what is pasted reads like what was seen. */
export function detailText(session: Session, d: SessionDetail): string {
  const started = d.started ? new Date(d.started).getTime() : NaN;
  const ended = d.last_active ? new Date(d.last_active).getTime() : NaN;
  const dur = !isNaN(started) && !isNaN(ended) ? fmtDuration(ended - started) : "";
  const win = windowFor(d.models, d.context_window);
  const lines: string[] = [];
  lines.push(d.title || session.title);
  lines.push(`Session ${d.id} (${session.agent})`);
  if (d.first_prompt) lines.push("", "Started with:", d.first_prompt);
  if (d.last_user && d.last_user !== d.first_prompt) lines.push("", "Last prompt:", d.last_user);
  if (d.last_assistant) lines.push("", "Last reply:", d.last_assistant);
  lines.push("");
  if (d.started) {
    lines.push(`When: ${fmtWhen(d.started)}${d.last_active && d.last_active !== d.started ? ` → ${fmtWhen(d.last_active)}` : ""}${dur ? ` (${dur})` : ""}`);
  }
  const where = d.cwd ?? session.project_path;
  if (where) lines.push(`Where: ${where}${(d.branch ?? session.branch) ? ` on ${d.branch ?? session.branch}` : ""}`);
  if (d.models.length) {
    lines.push(`Model: ${d.models.join(" → ")}${d.effort ? `, ${d.effort} effort` : ""}${d.permission_mode ? `, ${d.permission_mode}` : ""}`);
  }
  lines.push(`Messages: ${d.user_messages} prompts, ${d.assistant_messages} replies${d.tool_calls ? `, ${d.tool_calls} tool calls` : ""}${d.compactions ? `, compacted ${d.compactions}×` : ""}`);
  if (d.context_tokens !== null) {
    lines.push(`Context: ${fmtTok(d.context_tokens)}${win ? ` of ${fmtTok(win)}` : ""}${d.output_tokens ? `, ${fmtTok(d.output_tokens)} written` : ""}`);
  }
  if (d.tools.length) lines.push(`Tools: ${d.tools.map((t) => `${t.name} ×${t.count}`).join(", ")}`);
  if (d.files.length) lines.push("", "Files touched:", ...d.files.map((f) => `  ${f}`));
  if (d.pr_links.length) lines.push("", "Pull requests:", ...d.pr_links.map((u) => `  ${u}`));
  if (d.cli_version) lines.push("", `${session.agent} ${d.cli_version}`);
  return lines.join("\n");
}

export default function SessionFlyout({ session, detail }: {
  session: Session;
  /** Null while loading. */
  detail: SessionDetail | null;
}) {
  const d = detail;
  const started = d?.started ? new Date(d.started).getTime() : NaN;
  const ended = d?.last_active ? new Date(d.last_active).getTime() : NaN;
  const dur = !isNaN(started) && !isNaN(ended) ? fmtDuration(ended - started) : "";
  const win = d ? windowFor(d.models, d.context_window) : null;
  const ctxPct = d?.context_tokens && win ? Math.min(100, (d.context_tokens / win) * 100) : null;
  const title = d?.title || session.title;
  const [copied, setCopied] = useState(false);
  const copy = () => {
    if (!d) return;
    navigator.clipboard.writeText(detailText(session, d)).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    }).catch(() => {});
  };

  return (
    <div className="sfly" role="tooltip">
      <div className="sfly-head">
        <AgentIcon agent={session.agent} size={16} />
        <span className="sfly-title">{title}</span>
        {d && (
          <button className="icon-btn" title="Copy this summary" onClick={copy}>
            <Icon of={copied ? Check : Copy} size="sm" />
          </button>
        )}
      </div>

      {!d && <div className="sfly-loading">Reading the transcript…</div>}

      {d && (
        <>
          {d.first_prompt && (
            <div className="sfly-block">
              <div className="sfly-label">Started with</div>
              <div className="sfly-quote">{d.first_prompt}</div>
            </div>
          )}
          {(d.last_user || d.last_assistant) && d.last_user !== d.first_prompt && (
            <div className="sfly-block">
              <div className="sfly-label">Last exchange</div>
              {d.last_user && <div className="sfly-quote user">{d.last_user}</div>}
              {d.last_assistant && <div className="sfly-quote">{d.last_assistant}</div>}
            </div>
          )}
          {!d.first_prompt && !d.last_assistant && (
            <div className="sfly-block sfly-dim">No conversation recorded yet.</div>
          )}

          <div className="sfly-facts">
            {d.started && (
              <div className="sfly-fact">
                <Icon of={Clock} size="sm" />
                <span>
                  {fmtWhen(d.started)}
                  {d.last_active && d.last_active !== d.started && ` → ${fmtWhen(d.last_active)}`}
                  {dur && <span className="sfly-dim"> · {dur}</span>}
                </span>
              </div>
            )}
            {(d.cwd || session.project_path) && (
              <div className="sfly-fact">
                <Icon of={Folder} size="sm" />
                <span className="sfly-mono">{homeAbbrev(d.cwd ?? session.project_path)}</span>
                {(d.branch ?? session.branch) && (
                  <span className="sfly-chip"><Icon of={GitBranch} size="sm" />{d.branch ?? session.branch}</span>
                )}
              </div>
            )}
            {d.models.length > 0 && (
              <div className="sfly-fact">
                <BrandIcon name={brandForModel(d.models[0])} size={13} />
                <span>
                  {d.models.join(" → ")}
                  {d.effort && <span className="sfly-dim"> · {d.effort} effort</span>}
                  {d.permission_mode && <span className="sfly-dim"> · {d.permission_mode}</span>}
                </span>
              </div>
            )}
            <div className="sfly-fact">
              <Icon of={MessageSquare} size="sm" />
              <span>
                {d.user_messages} prompt{d.user_messages === 1 ? "" : "s"}, {d.assistant_messages} repl{d.assistant_messages === 1 ? "y" : "ies"}
                {d.tool_calls > 0 && <span className="sfly-dim"> · {d.tool_calls} tool calls</span>}
                {d.compactions > 0 && <span className="sfly-dim"> · compacted {d.compactions}×</span>}
              </span>
            </div>
            {d.context_tokens !== null && (
              <div className="sfly-fact">
                <Icon of={PieChart} size="sm" />
                <span className="sfly-ctx">
                  <span>
                    {fmtTok(d.context_tokens)} in context
                    {win && <span className="sfly-dim"> of {fmtTok(win)}</span>}
                    {d.output_tokens > 0 && <span className="sfly-dim"> · {fmtTok(d.output_tokens)} written</span>}
                  </span>
                  {ctxPct !== null && (
                    <span className="usage-track sfly-track">
                      <span
                        className={"usage-fill " + (ctxPct > 85 ? "sev-high" : ctxPct > 60 ? "sev-mid" : "sev-none")}
                        style={{ width: ctxPct + "%" }}
                      />
                    </span>
                  )}
                </span>
              </div>
            )}
            {d.tools.length > 0 && (
              <div className="sfly-fact">
                <Icon of={Hammer} size="sm" />
                <span className="sfly-dim">
                  {d.tools.map((t) => `${t.name} ×${t.count}`).join(", ")}
                </span>
              </div>
            )}
          </div>

          {d.files.length > 0 && (
            <div className="sfly-block">
              <div className="sfly-label"><Icon of={FileCode2} size="sm" /> Files touched</div>
              <div className="sfly-files">
                {d.files.map((f) => (
                  <div key={f} className="sfly-mono" title={f}>{shortPath(f, d.cwd)}</div>
                ))}
              </div>
            </div>
          )}
          {d.pr_links.length > 0 && (
            <div className="sfly-block">
              <div className="sfly-label"><Icon of={GitPullRequest} size="sm" /> Pull requests</div>
              {d.pr_links.map((u) => <div key={u} className="sfly-mono">{u}</div>)}
            </div>
          )}
          {d.cli_version && (
            <div className="sfly-foot sfly-dim">
              {session.agent} {d.cli_version}
              {session.forked ? " · forked" : ""}
              {session.background ? " · background" : ""}
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** A path relative to the session's directory where it is under it. */
function shortPath(p: string, cwd: string | null): string {
  if (cwd && p.startsWith(cwd + "/")) return p.slice(cwd.length + 1);
  return homeAbbrev(p);
}
