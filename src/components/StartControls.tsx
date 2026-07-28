import { useEffect, useState } from "react";
import { AgentChoice, ModelOption, agentChoices, providerModels } from "../ipc";
import AgentIcon from "./AgentIcon";

/** What to start, once a directory is chosen. */
export interface StartChoice {
  agentId: string;
  model: string | null;
  effort: string | null;
  /** Whether the agent will accept a pre-minted `--session-id`. Where false the
   *  id aiterm generates is a tab handle only and no panel is keyed to it. */
  mintsSessionId: boolean;
}

/**
 * Source, model and effort — the three decisions a new session needs.
 *
 * Shared by the ＋ menu and the empty pane. They used to disagree: the menu
 * asked, and the empty pane silently took the first installed agent on its
 * defaults, which is a different session from the one the menu would have
 * started and no way to tell from the button.
 *
 * Only installed agents are offered. The source row is hidden when there is
 * one, since a choice of one is furniture — but never when there are none,
 * because "no agent CLI found" is the most useful thing the panel can say.
 *
 * Model and effort default to blank, meaning "whatever the agent would do on
 * its own". That is the only honest default: any value picked here would be
 * aiterm choosing on your behalf. Effort narrows to the chosen *model* — Codex
 * publishes a different set per model, and it is not cosmetic (gpt-5.6-sol
 * offers `ultra`, gpt-5.5 stops at `xhigh`).
 */
export function useStartChoice() {
  const [agents, setAgents] = useState<AgentChoice[]>([]);
  const [agentId, setAgentId] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");

  useEffect(() => {
    agentChoices()
      .then((list) => {
        setAgents(list);
        setAgentId((cur) => cur || list[0]?.id || "");
      })
      .catch(() => setAgents([]));
  }, []);

  // Models fetched for API sources, keyed by source id. Those lists come from
  // the provider over the network, so they are pulled when a source is picked
  // rather than every time the picker opens — and cached, because reopening
  // the menu should not re-ask someone's API.
  const [apiModels, setApiModels] = useState<Record<string, ModelOption[] | "loading" | "error">>({});

  const agent = agents.find((a) => a.id === agentId) ?? null;
  const isApi = agentId.startsWith("api:");
  const fetched = apiModels[agentId];

  useEffect(() => {
    if (!isApi || fetched !== undefined) return;
    const providerId = agentId.slice("api:".length);
    setApiModels((m) => ({ ...m, [agentId]: "loading" }));
    providerModels(providerId)
      .then((ids) =>
        setApiModels((m) => ({
          ...m,
          [agentId]: ids.map((id) => ({
            id, display_name: id, efforts: [], default_effort: null,
          })),
        })),
      )
      .catch(() => setApiModels((m) => ({ ...m, [agentId]: "error" })));
  }, [agentId, isApi, fetched]);

  const models = isApi
    ? (Array.isArray(fetched) ? fetched : [])
    : (agent?.models ?? []);
  const efforts = models.find((m) => m.id === model)?.efforts ?? [];

  // Switching source invalidates both: a Claude alias is not a Codex slug.
  const pickAgent = (id: string) => { setAgentId(id); setModel(""); setEffort(""); };
  // Switching model keeps the effort only if the new model still offers it.
  const pickModel = (id: string) => {
    setModel(id);
    const next = models.find((m) => m.id === id)?.efforts ?? [];
    setEffort((cur) => (next.includes(cur) ? cur : ""));
  };

  const choice = (): StartChoice => ({
    agentId,
    model: model || null,
    effort: effort || null,
    mintsSessionId: agent?.mints_session_id ?? false,
  });

  return {
    agents, agent, agentId, model, effort, models, efforts,
    isApi, modelsState: fetched,
    pickAgent, pickModel, setEffort, choice,
    ready: agents.length > 0,
  };
}

type Ctl = ReturnType<typeof useStartChoice>;

export default function StartControls({ ctl }: { ctl: Ctl }) {
  const {
    agents, agent, agentId, model, effort, models, efforts,
    isApi, modelsState, pickAgent, pickModel, setEffort,
  } = ctl;
  return (
    <div className="ns-agents">
      {agents.length > 1 && (
        <div className="ns-agent-tabs">
          {agents.map((a) => (
            <button
              key={a.id}
              className={"ns-agent-tab" + (a.id === agentId ? " on" : "")}
              onClick={() => pickAgent(a.id)}
              title={a.display_name}
            >
              <AgentIcon agent={a.id} size={14} />
              <span>{a.display_name}</span>
            </button>
          ))}
        </div>
      )}
      <div className="ns-selects">
        <select
          className="ns-select"
          value={model}
          onChange={(e) => pickModel(e.target.value)}
          disabled={models.length === 0}
          title={models.length ? "Model" : "This source publishes no model list"}
        >
          <option value="">
            {isApi && modelsState === "loading" ? "Loading models…"
              : isApi && modelsState === "error" ? "Could not load models"
              : "Default model"}
          </option>
          {models.map((m) => (
            <option key={m.id} value={m.id}>{m.display_name}</option>
          ))}
        </select>
        <select
          className="ns-select"
          value={effort}
          onChange={(e) => setEffort(e.target.value)}
          disabled={efforts.length === 0}
          title={efforts.length ? "Effort" : "Pick a model first"}
        >
          <option value="">Default effort</option>
          {efforts.map((e) => (
            <option key={e} value={e}>{e}</option>
          ))}
        </select>
      </div>
      {agents.length === 0 && (
        <div className="empty-note">No agent or API source configured.</div>
      )}
      {isApi && (
        // Said here because it changes what the session *is*: the terminal is
        // still Claude Code, pointed at someone else's endpoint.
        <div className="ns-note">Runs Claude Code against {agent?.display_name}.</div>
      )}
    </div>
  );
}
