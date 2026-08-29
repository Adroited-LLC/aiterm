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
import Icon from "./Icon";
import { Plug, SquareTerminal } from "lucide-react";

export default function AgentIcon({ agent, size = 16 }: { agent: string; size?: number }) {
  const brand = brandForAgent(agent);
  if (brand) return <BrandIcon name={brand} size={size} className={`agent-icon ${agent}`} />;

  if (agent === "api") {
    // A socket: an endpoint you plug a key into, deliberately unlike any
    // vendor's mark so "this one is a remote model" reads instantly.
    return (
      <Icon of={Plug} px={size} className={`agent-icon ${agent}`} />
    );
  }

  // Anything else: a generic terminal, so an unknown source is still legible.
  return (
    <Icon of={SquareTerminal} px={size} className={`agent-icon ${agent}`} />
  );
}
