import { useState } from "react";
import { ClaudeLayer, claudeSaveLayer } from "../../ipc";
import { describeSaveError } from "./SettingsSection";

/** The raw per-layer editor: everything a row control cannot do — add a key,
 *  remove one, restructure the file — because it replaces the whole file
 *  rather than patching one path into it. Opens inline beneath its layer's
 *  row in the Files list, not as a modal; the panel is already a drill-down
 *  and a third stacked surface is one too many. */
export default function RawLayerEditor({
  layer,
  busy,
  setBusy,
  onSaved,
  onClose,
}: {
  layer: ClaudeLayer;
  /** Section-wide, same flag `SettingRow` gates on — this save and a row's
   *  save can target the same file, and the same collision-by-racing-
   *  ourselves problem applies here too. */
  busy: boolean;
  setBusy: (b: boolean) => void;
  onSaved: () => Promise<void>;
  onClose: () => void;
}) {
  // An absent layer has no bytes to seed from (`layer.text` is already ""),
  // but `{}` is the smallest valid settings file — starting there means
  // Save is how the file gets created, rather than needing a separate path
  // for "create" versus "edit".
  const initial = layer.present ? layer.text : "{}";

  const [text, setText] = useState(initial);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<{ collision: boolean; text: string } | null>(null);

  // Checked on every keystroke, not just on save — the point is to tell the
  // user their JSON is broken while they can still see what they typed, not
  // after a round trip to the backend confirms it.
  let parseError: string | null = null;
  try {
    JSON.parse(text);
  } catch (e) {
    parseError = e instanceof Error ? e.message : String(e);
  }

  const dirty = text !== initial;
  const canSave = dirty && !parseError && !saving && !busy;
  const disabled = saving || busy;

  async function save() {
    if (!canSave) return;
    setSaving(true);
    setBusy(true);
    setErr(null);
    try {
      // `layer.text` (not `text`) is the comparison token — the backend
      // checks it against the file's current bytes to detect a change since
      // this panel read it, which is exactly the collision Task 5 guards.
      await claudeSaveLayer(layer.path, text, layer.text);
      // The save just replaced this layer's bytes, so every layer's `text`
      // in the parent's view is now stale — a fresh read is the only
      // trustworthy next state, same as a row save.
      await onSaved();
    } catch (e) {
      setErr(describeSaveError(e));
    } finally {
      setSaving(false);
      setBusy(false);
    }
  }

  function cancel() {
    // Silent discard is fine when there is nothing to lose; once the user
    // has typed something different, a stray click should not be able to
    // throw it away without asking.
    if (dirty && !window.confirm("Discard changes to this file?")) return;
    onClose();
  }

  return (
    <div className="acfg-raw">
      {!layer.present && (
        <div className="acfg-note">This file does not exist yet — Save creates it.</div>
      )}
      <textarea
        className="acfg-raw-area"
        value={text}
        disabled={disabled}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
      />
      {parseError && <div className="acfg-err">{parseError}</div>}
      <div className="acfg-raw-actions">
        <button className="acfg-save" disabled={!canSave} onClick={save}>
          {saving ? "Saving…" : "Save"}
        </button>
        <button className="acfg-cancel" disabled={disabled} onClick={cancel}>
          Cancel
        </button>
      </div>
      {err &&
        (err.collision ? (
          <div className="acfg-collision">
            That file changed on disk since this panel read it — your edit was not saved.
            <button
              className="acfg-open"
              onClick={() => {
                // Cleared here rather than left for a reload to overwrite:
                // if the file's content ends up matching what this editor
                // already has, nothing downstream changes to naturally
                // dismiss the message.
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
    </div>
  );
}
