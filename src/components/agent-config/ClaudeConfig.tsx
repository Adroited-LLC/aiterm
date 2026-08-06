import { useState } from "react";
import { AgentDetection, homeAbbrev } from "../../ipc";
import SettingsSection from "./SettingsSection";
import InstructionsSection from "./InstructionsSection";
import McpSection from "./McpSection";
import SkillsSection from "./SkillsSection";

type Section = "settings" | "instructions" | "mcp" | "skills";

const TABS: [Section, string][] = [
  ["settings", "Settings"],
  ["instructions", "Instructions"],
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
  return (
    <div className="acfg">
      <div className="acfg-head">
        <button className="acfg-back" onClick={onBack}>← Agents</button>
        <span className="acfg-title">{agent.display_name}</span>
        <span className="acfg-ver">{agent.version ?? "installed"}</span>
        {agent.path && <span className="acfg-path">{homeAbbrev(agent.path)}</span>}
      </div>
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
      {section === "mcp" && <McpSection project={project} />}
      {section === "skills" && <SkillsSection project={project} />}
    </div>
  );
}
