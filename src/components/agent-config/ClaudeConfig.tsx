import { useState } from "react";
import { AgentDetection, homeAbbrev } from "../../ipc";
import SettingsSection from "./SettingsSection";
import InstructionsSection from "./InstructionsSection";
import McpSection from "./McpSection";
import SkillsSection from "./SkillsSection";
import HooksSection from "./HooksSection";
import Icon from "../Icon";
import { ChevronLeft } from "lucide-react";

type Section = "settings" | "instructions" | "hooks" | "mcp" | "skills";

const TABS: [Section, string][] = [
  ["settings", "Settings"],
  ["instructions", "Instructions"],
  ["hooks", "Hooks"],
  ["mcp", "MCP"],
  ["skills", "Skills"],
];

/** Everything Claude Code reads, inside one panel that says whose it is.
 *
 *  The buttons live in here rather than in the Agents list so the scoping is
 *  never in doubt: nothing in this panel is an aiterm setting. */
export default function ClaudeConfig({ agent, project, onBack }: {
  agent: AgentDetection;
  project: string | null;
  onBack: () => void;
}) {
  const [section, setSection] = useState<Section>("settings");
  // Not the engine-name check the abstraction exists to remove. That one asked
  // "does this engine have config worth a button", which is what Caps.config
  // answers and what the Agents pane gates on. This one is this component
  // asserting its own identity: every section below reads ~/.claude and
  // ~/.claude.json specifically, so the first other engine to declare
  // config: true would get Claude's files shown under its own name.
  //
  // The header stays, because the back link is the only way out of this panel.
  const claude = agent.id === "claude";
  return (
    <div>
      <div className="acfg-head">
        <button className="acfg-back" onClick={onBack}><Icon of={ChevronLeft} size="sm" /> Agents</button>
        <span className="acfg-title">{agent.display_name}</span>
        <span className="acfg-ver">{agent.version ?? "installed"}</span>
        {agent.path && <span className="acfg-path">{homeAbbrev(agent.path)}</span>}
      </div>
      {!claude ? (
        <div className="acfg-empty">No configuration reader for this engine yet.</div>
      ) : (
        <>
          <div className="acfg-tabs">
            {TABS.map(([id, label]) => (
              <button
                key={id}
                className={"acfg-tab" + (section === id ? " on" : "")}
                onClick={() => setSection(id)}
              >{label}</button>
            ))}
          </div>
          {section === "settings" && <SettingsSection project={project} />}
          {section === "instructions" && <InstructionsSection project={project} />}
          {section === "hooks" && <HooksSection project={project} />}
          {section === "mcp" && <McpSection project={project} />}
          {section === "skills" && <SkillsSection project={project} />}
        </>
      )}
    </div>
  );
}
