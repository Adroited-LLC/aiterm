import { useEffect, useState } from "react";
import { AgentChoice, ProviderView, agentChoices, providersList } from "../ipc";
import AgentIcon from "./AgentIcon";

/** What to start, once a directory is chosen. */
export interface StartChoice {
  agentId: string;
  model: string | null;
  effort: string | null;
  /** Whether the agent will accept a pre-minted `--session-id`. Where false the
   *  id aiterm generates is a tab handle only and no panel is keyed to it. */
  mintsSessionId: boolean;
  /** An API-provider model off a startup list, instead of an agent CLI's
   *  model. Runs OpenCode where it can (OpenRouter provider, opencode
   *  installed), aiterm's own chat console otherwise. */
  api: {
    providerId: string;
    providerName: string;
    modelId: string;
    /** OpenCode's catalog only resolves OpenRouter slugs, so only an
     *  OpenRouter provider may route there. */
    openrouter: boolean;
  } | null;
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
  /** Providers with a key and a non-empty startup list — the only ones the
   *  API dropdown has anything to say about. */
  const [providers, setProviders] = useState<ProviderView[]>([]);
  /** `JSON.stringify([providerId, modelId])`, or "" for none. JSON because a
   *  model id can contain any separator this could have picked. */
  const [apiModel, setApiModel] = useState("");

  useEffect(() => {
    agentChoices()
      .then((list) => {
        setAgents(list);
        setAgentId((cur) => cur || list[0]?.id || "");
      })
      .catch(() => setAgents([]));
    providersList()
      .then((list) =>
        setProviders(list.filter((p) => p.has_key && p.startup_models.length > 0)),
      )
      .catch(() => {});
  }, []);

  const agent = agents.find((a) => a.id === agentId) ?? null;
  const models = agent?.models ?? [];
  const efforts = models.find((m) => m.id === model)?.efforts ?? [];

  // Switching source invalidates both: a Claude alias is not a Codex slug.
  // It also ends an API pick — the tabs and the dropdown are one axis.
  const pickAgent = (id: string) => {
    setAgentId(id); setModel(""); setEffort(""); setApiModel("");
  };
  // Switching model keeps the effort only if the new model still offers it.
  const pickModel = (id: string) => {
    setModel(id);
    const next = models.find((m) => m.id === id)?.efforts ?? [];
    setEffort((cur) => (next.includes(cur) ? cur : ""));
  };

  const choice = (): StartChoice => {
    let api: StartChoice["api"] = null;
    if (apiModel) {
      const [providerId, modelId] = JSON.parse(apiModel) as [string, string];
      const p = providers.find((x) => x.id === providerId);
      api = {
        providerId,
        providerName: p?.name ?? providerId,
        modelId,
        openrouter: (p?.base_url ?? "").includes("openrouter.ai"),
      };
    }
    return {
      agentId,
      model: model || null,
      effort: effort || null,
      mintsSessionId: agent?.mints_session_id ?? false,
      api,
    };
  };

  return {
    agents, agentId, model, effort, models, efforts, providers, apiModel,
    pickAgent, pickModel, setEffort, setApiModel, choice,
    ready: agents.length > 0,
  };
}

type Ctl = ReturnType<typeof useStartChoice>;

export default function StartControls({ ctl }: { ctl: Ctl }) {
  const {
    agents, agentId, model, effort, models, efforts, providers, apiModel,
    pickAgent, pickModel, setEffort, setApiModel,
  } = ctl;
  const apiPicked = apiModel !== "";
  return (
    <div className="ns-agents">
      {agents.length > 1 && (
        <div className="ns-agent-tabs">
          {agents.map((a) => (
            <button
              key={a.id}
              className={"ns-agent-tab" + (a.id === agentId && !apiPicked ? " on" : "")}
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
          disabled={models.length === 0 || apiPicked}
          title={
            apiPicked
              ? "An API model is picked instead"
              : models.length
                ? "Model"
                : "This source publishes no model list"
          }
        >
          <option value="">Default model</option>
          {models.map((m) => (
            <option key={m.id} value={m.id}>{m.display_name}</option>
          ))}
        </select>
        <select
          className="ns-select"
          value={effort}
          onChange={(e) => setEffort(e.target.value)}
          disabled={efforts.length === 0 || apiPicked}
          title={
            apiPicked ? "An API model is picked instead" : efforts.length ? "Effort" : "Pick a model first"
          }
        >
          <option value="">Default effort</option>
          {efforts.map((e) => (
            <option key={e} value={e}>{e}</option>
          ))}
        </select>
        {/* Providers appear once something is on their startup list — the
            shortlist built in Settings → Model access. */}
        {providers.length > 0 && (
          <select
            className="ns-select"
            value={apiModel}
            onChange={(e) => setApiModel(e.target.value)}
            title="A model from a configured provider"
          >
            <option value="">API model</option>
            {providers.map((p) => (
              <optgroup key={p.id} label={p.name}>
                {p.startup_models.map((m) => (
                  <option key={m} value={JSON.stringify([p.id, m])}>{m}</option>
                ))}
              </optgroup>
            ))}
          </select>
        )}
      </div>
      {apiPicked && (
        <div className="empty-note">
          Runs OpenCode on this model — or aiterm's chat console where
          OpenCode can't take it.
        </div>
      )}
      {agents.length === 0 && (
        <div className="empty-note">No agent CLI found on this machine.</div>
      )}
    </div>
  );
}
