import { useEffect, useState } from "react";
import { AgentChoice, ProviderView, agentChoices, providersList } from "../ipc";
import AgentIcon from "./AgentIcon";

/**
 * What to start, once a directory is chosen.
 *
 * One thing or the other, and the type says so. It used to be a single object
 * with an agent id, a model, an effort and an optional `api` sub-object beside
 * them — a shape in which "an agent AND an API model" was representable, and
 * which of the two you meant was decided at the call site by whichever field it
 * happened to test first.
 *
 * Two fields are gone rather than moved. `mintsSessionId` and the `openrouter`
 * flag were the frontend answering questions that belong to the engine: whether
 * an id can be pre-minted, and whether OpenCode's catalog can resolve a slug.
 * The resolver answers both now, and its plan reports what it decided.
 */
export type StartChoice =
  | { kind: "agent"; agentId: string; model: string | null; effort: string | null }
  /** A model off a provider's startup list. Which engine runs it — OpenCode,
   *  or aiterm's own chat console — is `launch.rs`'s answer, not this one's. */
  | { kind: "api"; providerId: string; modelId: string };

/** The source tab that stands for "a model from Model access" — one tab beside
 *  the agent CLIs rather than a dropdown that appears only once it has
 *  something to say. Not a backend id: no engine calls itself this, and the
 *  choice it produces is `kind: "api"`, which the resolver routes. */
export const API_SOURCE = "api";

/**
 * Source, model and effort — the three decisions a new session needs.
 *
 * Shared by the ＋ menu and the empty pane. They used to disagree: the menu
 * asked, and the empty pane silently took the first installed agent on its
 * defaults, which is a different session from the one the menu would have
 * started and no way to tell from the button.
 *
 * Only installed agents are offered, plus the API source, which is always
 * there: it is the one source whose setup lives inside aiterm, so its tab is
 * where the setup starts when there is none.
 *
 * Model and effort default to blank, meaning "whatever the agent would do on
 * its own". That is the only honest default: any value picked here would be
 * aiterm choosing on your behalf. Effort narrows to the chosen *model* — Codex
 * publishes a different set per model, and it is not cosmetic (gpt-5.6-sol
 * offers `ultra`, gpt-5.5 stops at `xhigh`).
 *
 * `reloadKey`: change it to re-read the agent and provider lists. The ＋ menu
 * remounts every open and needs nothing; the empty pane's copy lives as long
 * as the window does, and would otherwise keep offering setup after it was
 * done in Settings.
 */
export function useStartChoice(reloadKey?: unknown) {
  const [agents, setAgents] = useState<AgentChoice[]>([]);
  const [agentId, setAgentId] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  /** Every configured provider. The API tab offers only the ones with a key
   *  and a non-empty startup list; the rest are what its setup is about — a
   *  provider half-configured is the state most worth naming. */
  const [allProviders, setAllProviders] = useState<ProviderView[]>([]);
  const providers = allProviders.filter((p) => p.has_key && p.startup_models.length > 0);
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
    providersList().then(setAllProviders).catch(() => {});
  }, [reloadKey]);

  const agent = agents.find((a) => a.id === agentId) ?? null;
  const isApi = agentId === API_SOURCE;
  /** The API source has something to start: at least one provider is fully
   *  set up. Until then its tab opens the setup instead of selecting. */
  const apiReady = providers.length > 0;
  const models = agent?.models ?? [];
  const efforts = models.find((m) => m.id === model)?.efforts ?? [];

  /** The first starred model, as the API tab's default pick — a source with
   *  nothing chosen would start nothing. */
  const firstApiModel = () => {
    const p = providers[0];
    return p ? JSON.stringify([p.id, p.startup_models[0]]) : "";
  };

  // Switching source invalidates both: a Claude alias is not a Codex slug.
  // Moving onto the API tab picks its first model; moving off clears it.
  const pickAgent = (id: string) => {
    setAgentId(id); setModel(""); setEffort("");
    setApiModel(id === API_SOURCE ? firstApiModel() : "");
  };
  // Switching model keeps the effort only if the new model still offers it.
  const pickModel = (id: string) => {
    setModel(id);
    const next = models.find((m) => m.id === id)?.efforts ?? [];
    setEffort((cur) => (next.includes(cur) ? cur : ""));
  };

  const choice = (): StartChoice => {
    if (isApi && apiModel) {
      const [providerId, modelId] = JSON.parse(apiModel) as [string, string];
      return { kind: "api", providerId, modelId };
    }
    return { kind: "agent", agentId, model: model || null, effort: effort || null };
  };

  return {
    agents, agentId, model, effort, models, efforts, providers, allProviders, apiModel,
    isApi, apiReady,
    pickAgent, pickModel, setEffort, setApiModel, choice,
    ready: isApi ? apiReady && apiModel !== "" : agents.length > 0,
  };
}

type Ctl = ReturnType<typeof useStartChoice>;

/**
 * What the API tab is waiting on, and where its setup should land.
 *
 * Three states, each named with the provider it is about, so the tab's
 * tooltip and the settings window it opens both point at the one step left:
 *
 * - a key but an empty shortlist: the common one — Test worked, nothing was
 *   starred, and nothing said stars were the point;
 * - a provider with no key;
 * - no provider at all.
 */
function apiSetup(all: ProviderView[]): { text: string; provider?: string } {
  const unstarred = all.find((p) => p.has_key && p.startup_models.length === 0);
  if (unstarred) {
    return {
      text: `${unstarred.name} is connected but has no startup models yet — click to pick some`,
      provider: unstarred.id,
    };
  }
  const keyless = all.find((p) => !p.has_key);
  if (keyless) {
    return { text: `${keyless.name} has no API key yet — click to add one`, provider: keyless.id };
  }
  return { text: "Run a model over an API — OpenRouter, xAI, OpenAI… Click to add a provider" };
}

interface Props {
  ctl: Ctl;
  /** Open Settings → Model access, on this provider when one is named. The
   *  API tab calls this instead of selecting while there is nothing to
   *  select. Without it the tab still says what is missing; it just cannot
   *  take you there. */
  onOpenModelAccess?: (providerId?: string) => void;
}

export default function StartControls({ ctl, onOpenModelAccess }: Props) {
  const {
    agents, agentId, model, effort, models, efforts, providers, allProviders, apiModel,
    isApi, apiReady, pickAgent, pickModel, setEffort, setApiModel,
  } = ctl;
  const setup = apiReady ? null : apiSetup(allProviders);
  const onApiTab = () => {
    if (apiReady) pickAgent(API_SOURCE);
    else onOpenModelAccess?.(setup?.provider);
  };
  return (
    <div className="ns-agents">
      <div className="ns-agent-tabs">
        {agents.map((a) => (
          <button
            key={a.id}
            className={"ns-agent-tab " + a.id + (a.id === agentId ? " on" : "")}
            onClick={() => pickAgent(a.id)}
            title={a.display_name}
          >
            <AgentIcon agent={a.id} size={14} />
            <span>{a.display_name}</span>
          </button>
        ))}
        <button
          className={"ns-agent-tab" + (isApi ? " on" : "") + (apiReady ? "" : " setup")}
          onClick={onApiTab}
          title={setup?.text ?? "A model from a configured provider"}
        >
          <AgentIcon agent={API_SOURCE} size={14} />
          <span>API</span>
          {!apiReady && <span className="ns-tab-setup">set up</span>}
        </button>
      </div>
      <div className="ns-selects">
        {isApi ? (
          <select
            className="ns-select"
            value={apiModel}
            onChange={(e) => setApiModel(e.target.value)}
            title="A model from a configured provider"
          >
            {providers.map((p) => (
              <optgroup key={p.id} label={p.name}>
                {p.startup_models.map((m) => (
                  <option key={m} value={JSON.stringify([p.id, m])}>{m}</option>
                ))}
              </optgroup>
            ))}
          </select>
        ) : (
          <select
            className="ns-select"
            value={model}
            onChange={(e) => pickModel(e.target.value)}
            disabled={models.length === 0}
            title={models.length ? "Model" : "This source publishes no model list"}
          >
            <option value="">Default model</option>
            {models.map((m) => (
              <option key={m.id} value={m.id}>{m.display_name}</option>
            ))}
          </select>
        )}
        <select
          className="ns-select"
          value={effort}
          onChange={(e) => setEffort(e.target.value)}
          disabled={isApi || efforts.length === 0}
          title={
            isApi ? "API models take no effort setting" : efforts.length ? "Effort" : "Pick a model first"
          }
        >
          <option value="">Default effort</option>
          {efforts.map((e) => (
            <option key={e} value={e}>{e}</option>
          ))}
        </select>
      </div>
      {isApi && (
        <div className="empty-note">
          Runs in OpenCode where it can, and aiterm's own chat console
          otherwise. Either way the conversation is saved and resumable.
          {onOpenModelAccess && (
            <>
              {" "}
              <button className="linkish" onClick={() => onOpenModelAccess(providers[0]?.id)}>
                Manage models
              </button>
            </>
          )}
        </div>
      )}
      {agents.length === 0 && !isApi && (
        <div className="empty-note">
          No agent CLI installed — install claude, codex or grok, or use the
          API tab.
        </div>
      )}
    </div>
  );
}
