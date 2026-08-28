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
/**
 * `reloadKey`: change it to re-read the agent and provider lists. The ＋ menu
 * remounts every open and needs nothing; the empty pane's copy lives as long
 * as the window does, and would otherwise keep saying "no startup models"
 * after they were picked in Settings.
 */
export function useStartChoice(reloadKey?: unknown) {
  const [agents, setAgents] = useState<AgentChoice[]>([]);
  const [agentId, setAgentId] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  /** Every configured provider. The dropdown offers only the ones with a
   *  key and a non-empty startup list; the rest are what the setup note is
   *  about — a provider half-configured is the state most worth explaining. */
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

  // The dropdown and the agent tabs are one axis — picking a model clears the
  // agent selection and vice versa — so the choice is whichever one is set.
  const choice = (): StartChoice => {
    if (apiModel) {
      const [providerId, modelId] = JSON.parse(apiModel) as [string, string];
      return { kind: "api", providerId, modelId };
    }
    return { kind: "agent", agentId, model: model || null, effort: effort || null };
  };

  return {
    agents, agentId, model, effort, models, efforts, providers, allProviders, apiModel,
    pickAgent, pickModel, setEffort, setApiModel, choice,
    ready: agents.length > 0,
  };
}

type Ctl = ReturnType<typeof useStartChoice>;

/**
 * Why the API dropdown is not showing, and the one step that would make it.
 *
 * The dropdown lists providers with a key *and* a startup shortlist, and it
 * used to vanish silently when neither existed — which from the menu looks
 * like "no API way in", not "one thing left to do". Three states, each named
 * with the provider it is about and a button that lands on it:
 *
 * - a key but an empty shortlist: the common one — Test worked, nothing was
 *   starred, and nothing said stars were the point;
 * - a provider with no key;
 * - no provider at all.
 */
function setupNote(all: ProviderView[]): { text: string; action: string; provider?: string } {
  const unstarred = all.find((p) => p.has_key && p.startup_models.length === 0);
  if (unstarred) {
    return {
      text: `${unstarred.name} is connected but has no startup models yet.`,
      action: "Pick models",
      provider: unstarred.id,
    };
  }
  const keyless = all.find((p) => !p.has_key);
  if (keyless) {
    return { text: `${keyless.name} has no API key yet.`, action: "Add key", provider: keyless.id };
  }
  return { text: "Or run any API model — OpenRouter, xAI, OpenAI…", action: "Add a provider" };
}

interface Props {
  ctl: Ctl;
  /** Open Settings → Model access, on this provider when one is named. When
   *  absent the note still says what is missing; it just cannot take you
   *  there. */
  onOpenModelAccess?: (providerId?: string) => void;
}

export default function StartControls({ ctl, onOpenModelAccess }: Props) {
  const {
    agents, agentId, model, effort, models, efforts, providers, allProviders, apiModel,
    pickAgent, pickModel, setEffort, setApiModel,
  } = ctl;
  const apiPicked = apiModel !== "";
  const setup = providers.length === 0 ? setupNote(allProviders) : null;
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
          Runs in OpenCode where it can, and aiterm's own chat console
          otherwise. Either way the conversation is saved and resumable.
        </div>
      )}
      {setup && (
        <div className="ns-setup">
          <span>{setup.text}</span>
          {onOpenModelAccess && (
            <button
              className="act-btn"
              title="Settings → Model access"
              onClick={() => onOpenModelAccess(setup.provider)}
            >{setup.action}</button>
          )}
        </div>
      )}
      {agents.length === 0 && (
        <div className="empty-note">
          No agent CLI installed — install claude, codex or opencode, or start
          from an API model above.
        </div>
      )}
    </div>
  );
}
