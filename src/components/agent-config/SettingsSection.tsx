import { useCallback, useEffect, useState } from "react";
import {
  ClaudeLayer,
  ClaudeSaveError,
  ClaudeSetting,
  ClaudeSettingsView,
  claudeSetKey,
  claudeSettings,
  homeAbbrev,
  openPath,
} from "../../ipc";

const LAYER_LABEL: Record<string, string> = {
  user: "user",
  project: "project",
  projectLocal: "project local",
  injected: "aiterm",
};

function show(v: unknown): string {
  if (typeof v === "string") return v;
  return JSON.stringify(v);
}

/** The shapes this panel will build a control for. Anything else — a nested
 *  object, an array with a non-string element — falls back to the raw editor;
 *  a bespoke control per shape is not worth it for the settings that show up
 *  in practice. */
type Editable =
  | { kind: "string"; value: string }
  | { kind: "number"; value: number }
  | { kind: "boolean"; value: boolean }
  | { kind: "array"; value: string[] };

function classify(v: unknown): Editable | null {
  if (typeof v === "string") return { kind: "string", value: v };
  if (typeof v === "number") return { kind: "number", value: v };
  if (typeof v === "boolean") return { kind: "boolean", value: v };
  if (Array.isArray(v) && v.every((x) => typeof x === "string")) {
    return { kind: "array", value: v as string[] };
  }
  return null;
}

/** The row's draft, derived fresh from the setting in force. Used both to
 *  seed a row and to reset it after a save — a save always ends in a full
 *  reload, and the draft has to fall back in step with it. */
function draftFrom(e: Editable | null): { text: string; bool: boolean; items: string[] } {
  if (!e) return { text: "", bool: false, items: [] };
  switch (e.kind) {
    case "string":
      return { text: e.value, bool: false, items: [] };
    case "number":
      return { text: String(e.value), bool: false, items: [] };
    case "boolean":
      return { text: "", bool: e.value, items: [] };
    case "array":
      return { text: "", bool: false, items: e.value };
  }
}

/** `claudeSetKey` rejects with the backend's `SaveError`, serialised as
 *  `{ kind, detail }` — but a rejection can in principle be anything JS can
 *  throw, so this stays defensive rather than assuming the shape. */
function describeSaveError(e: unknown): { collision: boolean; text: string } {
  const se = e as Partial<ClaudeSaveError> & { detail?: string } | null;
  if (se && typeof se === "object") {
    if (se.kind === "collision") return { collision: true, text: "" };
    if (typeof se.detail === "string") return { collision: false, text: se.detail };
    if (typeof se.kind === "string") return { collision: false, text: se.kind };
  }
  return { collision: false, text: String(e) };
}

function SettingRow({
  s,
  layer,
  onSaved,
}: {
  s: ClaudeSetting;
  layer: ClaudeLayer | undefined;
  onSaved: () => Promise<void>;
}) {
  const editable = classify(s.effective);

  const [text, setText] = useState(() => draftFrom(editable).text);
  const [bool, setBool] = useState(() => draftFrom(editable).bool);
  const [items, setItems] = useState(() => draftFrom(editable).items);
  const [newItem, setNewItem] = useState("");
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<{ collision: boolean; text: string } | null>(null);

  // A reload replaces the whole view, so this key's `effective` is a new
  // value — the draft has to follow it rather than sit on what used to be
  // in force. Keyed on the value itself, not just s.key, so a save's own
  // reload clears the draft it just committed.
  useEffect(() => {
    const d = draftFrom(editable);
    setText(d.text);
    setBool(d.bool);
    setItems(d.items);
    setNewItem("");
    setErr(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.key, JSON.stringify(s.effective)]);

  if (!editable || !layer) {
    // Unsupported shape, or the winning layer is somehow missing from
    // `view.layers` — either way this row does not get a guess at a control.
    return (
      <div className="acfg-set">
        <span className="acfg-key">{s.key}</span>
        <span className="acfg-val">{show(s.effective)}</span>
        <span className="acfg-src">{LAYER_LABEL[s.winner] ?? s.winner}</span>
        <span className="acfg-note">Edit in the raw editor</span>
        {s.setIn.length > 1 && (
          <div className="acfg-over">
            also set in{" "}
            {s.setIn.slice(0, -1).map((x) => LAYER_LABEL[x.layer] ?? x.layer).join(", ")}
            {s.merged ? " — merged, all apply" : " — overridden"}
          </div>
        )}
      </div>
    );
  }

  const dirty =
    editable.kind === "string"
      ? text !== editable.value
      : editable.kind === "number"
        ? text !== String(editable.value)
        : editable.kind === "boolean"
          ? bool !== editable.value
          : JSON.stringify(items) !== JSON.stringify(editable.value);

  const numValid = editable.kind !== "number" || (text.trim() !== "" && !Number.isNaN(Number(text)));
  const canSave = dirty && numValid && !saving;

  function cancel() {
    const d = draftFrom(editable);
    setText(d.text);
    setBool(d.bool);
    setItems(d.items);
    setNewItem("");
    setErr(null);
  }

  async function save() {
    if (!editable || !layer) return;
    const value =
      editable.kind === "string"
        ? text
        : editable.kind === "number"
          ? Number(text)
          : editable.kind === "boolean"
            ? bool
            : items;
    setSaving(true);
    setErr(null);
    try {
      await claudeSetKey(layer.path, s.key, value, layer.text);
      // The file just changed under us — every layer's `text` is now stale,
      // so the only correct next state is whatever a fresh read says, not a
      // locally patched guess.
      await onSaved();
    } catch (e) {
      setErr(describeSaveError(e));
    } finally {
      setSaving(false);
    }
  }

  function addItem() {
    const v = newItem.trim();
    if (!v) return;
    setItems([...items, v]);
    setNewItem("");
  }

  return (
    <div className="acfg-set">
      <span className="acfg-key">{s.key}</span>

      {editable.kind === "string" && (
        <input
          className="acfg-edit"
          type="text"
          value={text}
          disabled={saving}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && canSave && save()}
        />
      )}
      {editable.kind === "number" && (
        <input
          className="acfg-edit"
          type="number"
          value={text}
          disabled={saving}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && canSave && save()}
        />
      )}
      {editable.kind === "boolean" && (
        <label className="acfg-edit acfg-bool">
          <input
            type="checkbox"
            checked={bool}
            disabled={saving}
            onChange={(e) => setBool(e.target.checked)}
          />
        </label>
      )}
      {editable.kind === "array" && (
        <div className="acfg-edit acfg-chips">
          {items.map((it, i) => (
            <span className="acfg-chip" key={`${it}-${i}`}>
              {it}
              <button
                type="button"
                disabled={saving}
                onClick={() => setItems(items.filter((_, j) => j !== i))}
              >
                ×
              </button>
            </span>
          ))}
          <input
            type="text"
            placeholder="add…"
            value={newItem}
            disabled={saving}
            onChange={(e) => setNewItem(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && addItem()}
          />
        </div>
      )}

      <span className="acfg-src">{LAYER_LABEL[s.winner] ?? s.winner}</span>

      {dirty && (
        <>
          <button className="acfg-save" disabled={!canSave} onClick={save}>
            {saving ? "Saving…" : "Save"}
          </button>
          <button className="acfg-cancel" disabled={saving} onClick={cancel}>
            Cancel
          </button>
        </>
      )}

      {err &&
        (err.collision ? (
          <div className="acfg-collision">
            That file changed on disk since this panel read it — your edit was not saved.
            <button
              className="acfg-open"
              onClick={() => {
                // Clearing here, not just via the reset effect: if this key's
                // value on disk is unchanged, `s.effective` comes back
                // identical and that effect never re-fires, leaving the
                // refusal on screen after a reload that actually worked.
                setErr(null);
                onSaved();
              }}
            >
              Reload
            </button>
          </div>
        ) : (
          <div className="acfg-err">{err.text}</div>
        ))}

      {s.setIn.length > 1 && (
        <div className="acfg-over">
          also set in{" "}
          {s.setIn.slice(0, -1).map((x) => LAYER_LABEL[x.layer] ?? x.layer).join(", ")}
          {s.merged ? " — merged, all apply" : " — overridden"}
        </div>
      )}
    </div>
  );
}

/** The layers, then every setting grouped by concern.
 *
 *  A setting shows the value in force and the file that set it; when more than
 *  one file sets it, the other setters are listed too — "project overrides
 *  user" is the sentence this section exists to make sayable, and for the keys
 *  Claude collects additively the sentence is "both apply" instead. */
export default function SettingsSection({ project }: { project: string | null }) {
  const [view, setView] = useState<ClaudeSettingsView | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Shared by the mount fetch and every row's post-save refresh: a save
  // invalidates every layer's `text`, so the only safe follow-up is asking
  // the backend again, never patching what's already in state.
  const reload = useCallback(async () => {
    try {
      const v = await claudeSettings(project);
      setView(v);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [project]);

  useEffect(() => {
    reload();
  }, [reload]);

  if (error) return <div className="acfg-err">{error}</div>;
  if (!view) return <div className="acfg-empty">Reading…</div>;

  // Groups in the order the backend asked for, then any concern it did not
  // mention. `concern::of` falls everything unrecognised into "Other", which is
  // always in `order` — but this panel's promise is that a setting in effect is
  // always visible, so it does not rely on that holding. A concern that appears
  // out of nowhere gets shown, not dropped.
  const named = view.order.filter((g) => view.settings.some((s) => s.concern === g));
  const extra = [...new Set(view.settings.map((s) => s.concern))].filter(
    (g) => !view.order.includes(g),
  );
  const groups = [...named, ...extra];

  return (
    <div>
      <div className="acfg-grp">Files</div>
      {view.layers.map((l) => (
        <div key={l.id} className="acfg-file">
          <span className="acfg-file-tag">{LAYER_LABEL[l.id] ?? l.id}</span>
          <span className={"acfg-file-path" + (l.present ? "" : " gone")}>
            {homeAbbrev(l.path)}
          </span>
          {l.present ? (
            <button className="acfg-open" onClick={() => openPath(l.path).catch(() => {})}>
              Open
            </button>
          ) : (
            <span className="acfg-file-state">not present</span>
          )}
          {l.error && <div className="acfg-err">{l.error}</div>}
        </div>
      ))}

      <div className="acfg-grp">Session start</div>
      <div className="acfg-flags">
        {view.injectedFlags.map((f) => (
          <code key={f} className="acfg-flag">{f}</code>
        ))}
        <div className="acfg-empty">
          These two are on every claude aiterm launches.
          {view.injectedFlags.some((f) => f.includes("skip-permissions")) &&
            " Permission prompts are off in these sessions."}
        </div>
        {/* The constants above are not the whole injected surface, and reading
            them as if they were answers "does aiterm set the model?" with a
            wrong no. These others depend on the launch, so they are described
            rather than listed — the panel cannot know one tab's argv. */}
        <div className="acfg-empty">
          Also added: <code className="acfg-flag">--settings</code> always, carrying aiterm's
          SessionStart hook (the aiterm layer above); and{" "}
          <code className="acfg-flag">--model</code>, <code className="acfg-flag">--effort</code>,{" "}
          <code className="acfg-flag">--session-id</code> when chosen in aiterm's start controls.
          A model chosen there beats <code className="acfg-flag">model</code> in settings.json,
          because command-line arguments outrank settings files.
        </div>
      </div>

      {groups.map((g) => (
        <div key={g}>
          <div className="acfg-grp">{g}</div>
          {view.settings.filter((s) => s.concern === g).map((s) => (
            <SettingRow
              key={s.key}
              s={s}
              layer={view.layers.find((l) => l.id === s.winner)}
              onSaved={reload}
            />
          ))}
        </div>
      ))}
    </div>
  );
}
