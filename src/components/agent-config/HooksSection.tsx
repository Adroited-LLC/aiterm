import { useCallback, useEffect, useState } from "react";
import {
  ClaudeHook,
  ClaudeHooksView,
  ClaudeLayerId,
  ClaudeSettingsView,
  claudeHooks,
  claudeSetKey,
  claudeSettings,
} from "../../ipc";
import { describeSaveError } from "./SettingsSection";

const EVENTS = [
  "SessionStart",
  "SessionEnd",
  "Stop",
  "SubagentStop",
  "PreToolUse",
  "PostToolUse",
  "UserPromptSubmit",
  "PreCompact",
  "Notification",
];

// `ClaudeHook.layer` arrives as the display label the backend already chose
// (`hooks.rs`'s `layer_label`), not the `ClaudeLayerId` a write needs — this
// is that mapping run in reverse, so Remove can find the file a given row
// actually came from.
const LABEL_TO_LAYER_ID: Record<string, ClaudeLayerId> = {
  user: "user",
  project: "project",
  "project local": "projectLocal",
  aiterm: "injected",
};

type RawHookEntry = { matcher?: string; hooks: { type: string; command: string }[] };
type RawHooks = Record<string, RawHookEntry[]>;

/** The `hooks` key out of a layer's raw text, defaulting to `{}` for a layer
 *  that has none — a write always sends a whole object back, and a layer with
 *  no hooks yet is something to build on, not an error. */
function currentHooks(text: string): RawHooks {
  try {
    const v = JSON.parse(text || "{}");
    const h = v && typeof v === "object" ? v.hooks : undefined;
    return h && typeof h === "object" ? (h as RawHooks) : {};
  } catch {
    return {};
  }
}

function withAdded(hooks: RawHooks, event: string, matcher: string, command: string): RawHooks {
  const entry: RawHookEntry = matcher
    ? { matcher, hooks: [{ type: "command", command }] }
    : { hooks: [{ type: "command", command }] };
  return { ...hooks, [event]: [...(hooks[event] ?? []), entry] };
}

/** Removes the first command matching `matcher` and `command`, then folds
 *  away anything the removal leaves empty — an entry with no commands left,
 *  or an event with no entries left — so a remove never leaves a trace of
 *  itself sitting in the file as `"hooks":[]`. */
function withRemoved(hooks: RawHooks, event: string, matcher: string | null, command: string): RawHooks {
  const list = hooks[event];
  if (!list) return hooks;
  let removed = false;
  const next: RawHookEntry[] = [];
  for (const entry of list) {
    if (!removed && (entry.matcher ?? null) === matcher) {
      const cmds = entry.hooks.filter((h) => {
        if (!removed && h.command === command) {
          removed = true;
          return false;
        }
        return true;
      });
      if (cmds.length > 0) next.push({ ...entry, hooks: cmds });
      continue;
    }
    next.push(entry);
  }
  if (next.length === 0) {
    const rest = { ...hooks };
    delete rest[event];
    return rest;
  }
  return { ...hooks, [event]: next };
}

/** Hooks, shown as what they are: shell commands that fire on their own at
 *  named events, additive across every layer that sets them. No row here may
 *  read as replacing another — aiterm's own hook is labelled and left alone,
 *  because it lives in aiterm's own `--settings` file and an editor that
 *  offered to touch it here would either fail or fight the writer. */
export default function HooksSection({ project }: { project: string | null }) {
  const [view, setView] = useState<ClaudeHooksView | null>(null);
  const [settings, setSettings] = useState<ClaudeSettingsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Section-wide, same reasoning as `SettingsSection`: a write invalidates
  // every layer's `text`, so a second write must not start before the first
  // write's reload has replaced both views it could read a stale token from.
  const [busy, setBusy] = useState(false);
  const [saveErr, setSaveErr] = useState<{ collision: boolean; text: string } | null>(null);

  const reload = useCallback(async () => {
    try {
      const [h, s] = await Promise.all([claudeHooks(project), claudeSettings(project)]);
      setView(h);
      setSettings(s);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [project]);

  useEffect(() => {
    reload();
  }, [reload]);

  const [event, setEvent] = useState<string>(EVENTS[0]);
  const [matcher, setMatcher] = useState("");
  const [command, setCommand] = useState("");
  const [confirmAdd, setConfirmAdd] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);

  if (error) return <div className="acfg-err">{error}</div>;
  if (!view || !settings) return <div className="acfg-empty">Reading…</div>;

  const userLayer = settings.layers.find((l) => l.id === "user");

  async function writeHooks(layerId: ClaudeLayerId, next: RawHooks) {
    const layer = settings!.layers.find((l) => l.id === layerId);
    if (!layer) return;
    setBusy(true);
    setSaveErr(null);
    try {
      await claudeSetKey(layer.path, "hooks", next, layer.text);
      // The write just changed this layer's bytes, so every layer's `text`
      // in state is stale — a fresh read is the only trustworthy base for
      // the next edit, same as the settings and raw-layer editors.
      await reload();
    } catch (e) {
      setSaveErr(describeSaveError(e));
    } finally {
      setBusy(false);
    }
  }

  async function addHook() {
    if (!userLayer || !command.trim()) return;
    const next = withAdded(currentHooks(userLayer.text), event, matcher.trim(), command.trim());
    await writeHooks("user", next);
    setConfirmAdd(false);
    setMatcher("");
    setCommand("");
  }

  async function removeHook(h: ClaudeHook) {
    const layerId = LABEL_TO_LAYER_ID[h.layer];
    const layer = settings!.layers.find((l) => l.id === layerId);
    if (!layer) return;
    const next = withRemoved(currentHooks(layer.text), h.event, h.matcher, h.command);
    await writeHooks(layerId, next);
    setConfirmRemove(null);
  }

  const errors = view.errors.map((e) => (
    <div key={e} className="acfg-err">{e}</div>
  ));
  const events = [...new Set(view.hooks.map((h) => h.event))];

  return (
    <div>
      {errors}
      {view.hooks.length === 0 && <div className="acfg-empty">No hooks configured.</div>}

      {events.map((ev) => (
        <div key={ev}>
          <div className="acfg-grp">{ev}</div>
          {view.hooks
            .filter((h) => h.event === ev)
            .map((h, i) => {
              const rowKey = `${h.layer}:${h.event}:${h.matcher ?? ""}:${h.command}:${i}`;
              return (
                <div key={rowKey} className="acfg-hook">
                  <span className="acfg-hook-matcher">{h.matcher ?? "any"}</span>
                  <span className="acfg-src">{h.layer}</span>
                  {/* Full width and let to wrap, never clipped — a truncated
                      shell command in a hook editor is a trap, not a summary. */}
                  <code className="acfg-hook-cmd">{h.command}</code>
                  {h.isAiterm ? (
                    <div className="acfg-note">
                      aiterm's own — lives in aiterm's own settings file, so your config stays untouched.
                    </div>
                  ) : confirmRemove === rowKey ? (
                    <span className="acfg-hook-confirm">
                      <span className="acfg-note">Remove this hook?</span>
                      <button
                        className="acfg-cancel acfg-danger"
                        disabled={busy}
                        onClick={() => removeHook(h)}
                      >
                        Remove
                      </button>
                      <button className="acfg-cancel" disabled={busy} onClick={() => setConfirmRemove(null)}>
                        Cancel
                      </button>
                    </span>
                  ) : (
                    <button className="acfg-open" disabled={busy} onClick={() => setConfirmRemove(rowKey)}>
                      Remove
                    </button>
                  )}
                </div>
              );
            })}
        </div>
      ))}

      <div className="acfg-grp">Add hook</div>
      <div className="acfg-note">
        {userLayer
          ? "Saved to the user layer, ~/.claude/settings.json — the one that applies everywhere."
          : "The user settings layer could not be read, so a hook cannot be added here."}
      </div>
      <div className="acfg-hook-form">
        <select
          className="acfg-edit"
          value={event}
          disabled={busy || !userLayer}
          onChange={(e) => setEvent(e.target.value)}
        >
          {EVENTS.map((e) => (
            <option key={e} value={e}>{e}</option>
          ))}
        </select>
        <input
          className="acfg-edit"
          type="text"
          placeholder="matcher (optional)"
          value={matcher}
          disabled={busy || !userLayer}
          onChange={(e) => {
            setMatcher(e.target.value);
            setConfirmAdd(false);
          }}
        />
        <input
          className="acfg-edit"
          type="text"
          placeholder="command"
          value={command}
          disabled={busy || !userLayer}
          onChange={(e) => {
            setCommand(e.target.value);
            setConfirmAdd(false);
          }}
        />
        {confirmAdd ? (
          <span className="acfg-hook-confirm">
            <span className="acfg-note">
              Will run on every {event}: <code>{command}</code>
            </span>
            <button className="acfg-save" disabled={busy} onClick={addHook}>
              Confirm add
            </button>
            <button className="acfg-cancel" disabled={busy} onClick={() => setConfirmAdd(false)}>
              Cancel
            </button>
          </span>
        ) : (
          <button
            className="acfg-save"
            disabled={busy || !userLayer || !command.trim()}
            onClick={() => setConfirmAdd(true)}
          >
            Add hook
          </button>
        )}
      </div>

      {saveErr &&
        (saveErr.collision ? (
          <div className="acfg-collision">
            That file changed on disk since this panel read it — your edit was not saved.
            <button
              className="acfg-open"
              onClick={() => {
                setSaveErr(null);
                reload();
              }}
            >
              Reload
            </button>
          </div>
        ) : (
          <div className="acfg-err">{saveErr.text}</div>
        ))}
    </div>
  );
}
