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
  // The bytes `text` was seeded from — captured at the same moment, from the
  // same read, so the two can never disagree about what "loaded" meant. Sent
  // to the backend as the collision token instead of the live `layer.text`
  // prop: that prop moves the instant a sibling row saves, and comparing a
  // stale draft against a fresh prop would let a genuine collision pass the
  // byte-exact check that exists specifically to catch it.
  const [loadedAt, setLoadedAt] = useState(initial);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<{ collision: boolean; text: string } | null>(null);
  // In-place confirm for Cancel, not `window.confirm`: that call is a native
  // dialog, and in the Tauri webview a native dialog blocks the whole
  // window — including the terminal tabs running behind this panel — until
  // it is dismissed. `SessionsPanel`'s delete confirm uses the same
  // swap-the-buttons shape for the same reason.
  const [confirmDiscard, setConfirmDiscard] = useState(false);

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
  // `initial` is recomputed from the live `layer` prop on every render, while
  // `loadedAt` was frozen at the moment the draft was seeded — so this is
  // true exactly when the file has moved under the draft, and it is true
  // before the user ever clicks Save, not just after a collision comes back.
  const stale = initial !== loadedAt;
  const canSave = dirty && !parseError && !saving && !busy && !stale;
  const disabled = saving || busy;

  async function save() {
    if (!canSave) return;
    setSaving(true);
    setBusy(true);
    setErr(null);
    try {
      // `loadedAt` (not the live `layer.text` prop) is the comparison token:
      // it is the exact read `text` was drafted from, captured in the same
      // state update. Comparing the draft against the live prop instead would
      // let a stale draft's save pass the backend's byte-exact collision
      // check whenever nothing *else* changed the file after this draft went
      // stale — silently overwriting whatever did change it (Fix 1).
      await claudeSaveLayer(layer.path, text, loadedAt);
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
    if (dirty) {
      setConfirmDiscard(true);
      return;
    }
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
        onChange={(e) => {
          setText(e.target.value);
          // A stale "discard?" prompt sitting over text the user has gone
          // back to editing is confusing — resuming the edit withdraws it.
          setConfirmDiscard(false);
        }}
      />
      {parseError && <div className="acfg-err">{parseError}</div>}
      {stale && (
        <div className="acfg-err">
          This file changed while you were editing. Saving will refuse; reload to get the current
          contents.
        </div>
      )}
      <div className="acfg-raw-actions">
        {confirmDiscard ? (
          <>
            <span className="acfg-note">Discard changes to this file?</span>
            <button
              className="acfg-cancel acfg-danger"
              onClick={() => {
                setConfirmDiscard(false);
                onClose();
              }}
            >
              Discard
            </button>
            <button className="acfg-cancel" onClick={() => setConfirmDiscard(false)}>
              Cancel
            </button>
          </>
        ) : (
          <>
            <button className="acfg-save" disabled={!canSave} onClick={save}>
              {saving ? "Saving…" : "Save"}
            </button>
            <button className="acfg-cancel" disabled={disabled} onClick={cancel}>
              Cancel
            </button>
          </>
        )}
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
                // Reseed both the draft and the token it is checked against
                // from what is currently loaded — otherwise the draft would
                // still read as `stale` (or a future save would still be
                // checked against bytes this reload just moved past) even
                // though the user asked to start over from the current file.
                setText(initial);
                setLoadedAt(initial);
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
