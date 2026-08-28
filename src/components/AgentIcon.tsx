/**
 * One mark per source, so a glance at a row says what produced it.
 *
 * Shared rather than local to the sessions panel: the same identity has to read
 * the same way in a session row, the start picker and the settings list, and
 * three copies of a switch on `agent` is how they drift apart.
 *
 * Engines draw their vendor's mark from the LobeHub set (`brand.ts`). The two
 * that are not a vendor keep their own glyphs: the API socket, which stands for
 * "a remote model from whichever provider", and the terminal for a source
 * nothing here recognises.
 */
import BrandIcon from "./BrandIcon";
import { brandForAgent } from "../brand";

export default function AgentIcon({ agent, size = 16 }: { agent: string; size?: number }) {
  const common = { className: `agent-icon ${agent}`, viewBox: "0 0 24 24", width: size, height: size };

  const brand = brandForAgent(agent);
  if (brand) return <BrandIcon name={brand} size={size} className={`agent-icon ${agent}`} />;

  if (agent === "api") {
    // A socket: an endpoint you plug a key into, deliberately unlike any
    // vendor's mark so "this one is a remote model" reads instantly.
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
