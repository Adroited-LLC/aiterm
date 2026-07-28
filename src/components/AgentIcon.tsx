/**
 * One mark per source, so a glance at a row says what produced it.
 *
 * Shared rather than local to the sessions panel: the same identity has to read
 * the same way in a session row, the start picker and the settings list, and
 * three copies of a switch on `agent` is how they drift apart.
 *
 * These are recognisable shapes, not the vendors' trademarks — an approximation
 * of a logo is still someone's logo, and this is a third-party app.
 */
/**
 * The four marks that exist, from whatever a row knows about itself.
 *
 * One function because the answer is not a straight switch on `agent`, and
 * every place that guessed at it locally got the interesting case wrong.
 * `agent` is the engine that owns the transcript, so a session started against
 * OpenRouter reports `"claude"` — truthfully, and identically to every other
 * row. `source` is what aiterm started it as, and it is the field that
 * distinguishes them; `agent` is the fallback for the majority of rows, which
 * aiterm did not start and which therefore have no source recorded.
 *
 * Returned as a small closed set rather than the raw id, because a source id is
 * `api:<provider-slug>` — unbounded, one per configured provider — and there is
 * exactly one "remote endpoint" mark for all of them.
 */
export type AgentKind = "claude" | "codex" | "api" | "generic";

export function agentKind(
  it: { agent?: string | null; source?: string | null },
): AgentKind {
  const id = it.source || it.agent || "";
  if (id.startsWith("api:") || id === "api") return "api";
  if (id === "codex" || id === "openai") return "codex";
  if (id === "claude") return "claude";
  return "generic";
}

/**
 * `agent` is any id: a backend id (`"api:openrouter"`), a session's `agent`, or
 * a kind straight from [`agentKind`]. It is normalised here rather than at each
 * call site — the start picker passed raw backend ids and so drew the generic
 * terminal for every configured API provider, which is precisely the "they all
 * look the same" complaint one layer up.
 */
export default function AgentIcon({ agent, size = 16 }: { agent: string; size?: number }) {
  const kind = agentKind({ agent });
  const common = { className: `agent-icon ${kind}`, viewBox: "0 0 24 24", width: size, height: size };

  if (kind === "claude") {
    // Starburst.
    return (
      <svg {...common}>
        <g fill="currentColor">
          {Array.from({ length: 12 }).map((_, i) => (
            <rect key={i} x="11.1" y="2" width="1.8" height="7" rx="0.9"
              transform={`rotate(${i * 30} 12 12)`} />
          ))}
        </g>
      </svg>
    );
  }

  if (kind === "codex") {
    // Interlocking knot — six-fold, echoing OpenAI's shape without copying it.
    return (
      <svg {...common} fill="none" stroke="currentColor" strokeWidth="1.6"
        strokeLinecap="round" strokeLinejoin="round">
        <path d="M12 3.2a4 4 0 0 1 3.9 3.1 4 4 0 0 1 2 6.8 4 4 0 0 1-3.9 5.6 4 4 0 0 1-6.9-1 4 4 0 0 1-2-6.8 4 4 0 0 1 3.9-5.6 4 4 0 0 1 3-2.1z" />
        <path d="M12 8.4v7.2M8.9 10.2l6.2 3.6M15.1 10.2l-6.2 3.6" opacity="0.55" />
      </svg>
    );
  }

  if (kind === "api") {
    // A socket: an endpoint you plug a key into, deliberately unlike either
    // CLI mark so "this one is a remote model" reads instantly.
    return (
      <svg {...common} fill="none" stroke="currentColor" strokeWidth="1.7"
        strokeLinecap="round" strokeLinejoin="round">
        <path d="M9 3v4M15 3v4" />
        <path d="M6.5 7h11v3.5a5.5 5.5 0 0 1-11 0V7z" />
        <path d="M12 16v5" />
      </svg>
    );
  }

  // Anything else: a generic terminal, so an unknown source is still legible.
  return (
    <svg {...common} fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M7 9l3 3-3 3M13 15h4" />
    </svg>
  );
}
