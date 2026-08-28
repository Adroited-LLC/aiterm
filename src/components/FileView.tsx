import { useCallback, useEffect, useRef, useState } from "react";
import { EditorView, keymap } from "@codemirror/view";
import { Compartment, EditorState } from "@codemirror/state";
import { basicSetup } from "codemirror";
import { indentWithTab } from "@codemirror/commands";
import { LanguageDescription, syntaxHighlighting } from "@codemirror/language";
import { classHighlighter } from "@lezer/highlight";
import { languages } from "@codemirror/language-data";
import { homeAbbrev, openPath, readTextFile, writeTextFile } from "../ipc";

/**
 * A file open in a center tab — CodeMirror over the real file on disk.
 *
 * The one thing this must never do is clobber somebody else's write: the
 * files it shows are exactly the files agents are editing in the terminal
 * beside it. So every save carries the mtime the buffer was loaded at, the
 * backend refuses a save over a file that moved past it, and the refusal is
 * surfaced as a choice (reload / overwrite) rather than resolved silently.
 * The project watcher's refresh tick keeps a clean buffer following the disk
 * for free; a dirty buffer is never touched behind the cursor.
 */
export default function FileView({
  path, active, refreshKey, onDirty,
}: {
  path: string;
  /** Whether this tab is the one on screen. CodeMirror measures nothing at
   *  display:none, so becoming visible schedules a re-measure. */
  active: boolean;
  /** The project watcher's tick — the same one the file tree refreshes on. */
  refreshKey: number;
  onDirty: (dirty: boolean) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  /** mtime the buffer content was loaded or last saved at — the save's
   *  compare-and-swap token. */
  const mtimeRef = useRef<number>(0);
  const dirtyRef = useRef(false);
  /** True while a programmatic replace is dispatching, so the update
   *  listener doesn't read our own reload as the user typing. */
  const settingRef = useRef(false);
  const [err, setErr] = useState<string | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [conflict, setConflict] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [saveErr, setSaveErr] = useState<string | null>(null);

  const markDirty = useCallback((d: boolean) => {
    dirtyRef.current = d;
    setDirty(d);
    onDirty(d);
  }, [onDirty]);

  /** Replace the buffer with what is on disk. */
  const loadFromDisk = useCallback(async () => {
    const view = viewRef.current;
    if (!view) return;
    try {
      const f = await readTextFile(path);
      mtimeRef.current = f.mtime_ms;
      setTruncated(f.truncated);
      setErr(null);
      settingRef.current = true;
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: f.content },
      });
      settingRef.current = false;
      markDirty(false);
      setConflict(false);
    } catch (e) {
      setErr(String(e));
    }
  }, [path, markDirty]);

  const save = useCallback(async (overwrite = false) => {
    const view = viewRef.current;
    if (!view) return;
    setSaveErr(null);
    try {
      const mtime = await writeTextFile(
        path,
        view.state.doc.toString(),
        overwrite ? null : mtimeRef.current,
      );
      mtimeRef.current = mtime;
      markDirty(false);
      setConflict(false);
    } catch (e) {
      if (String(e).includes("changed-on-disk")) setConflict(true);
      else setSaveErr(String(e));
    }
  }, [path, markDirty]);
  const saveRef = useRef(save);
  saveRef.current = save;

  useEffect(() => {
    if (!hostRef.current || viewRef.current) return;
    const lang = new Compartment();
    const lock = new Compartment();
    const view = new EditorView({
      state: EditorState.create({
        doc: "",
        extensions: [
          basicSetup,
          keymap.of([
            {
              key: "Mod-s",
              run: () => {
                saveRef.current();
                return true;
              },
            },
            indentWithTab,
          ]),
          syntaxHighlighting(classHighlighter),
          lang.of([]),
          lock.of([]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged && !settingRef.current && !dirtyRef.current) {
              markDirty(true);
            }
          }),
        ],
      }),
      parent: hostRef.current,
    });
    viewRef.current = view;
    (async () => {
      try {
        const f = await readTextFile(path);
        mtimeRef.current = f.mtime_ms;
        setTruncated(f.truncated);
        settingRef.current = true;
        view.dispatch({ changes: { from: 0, insert: f.content } });
        settingRef.current = false;
        if (f.truncated) {
          // A truncated read must not be saved back: it IS data loss.
          view.dispatch({
            effects: lock.reconfigure([
              EditorState.readOnly.of(true),
              EditorView.editable.of(false),
            ]),
          });
        }
      } catch (e) {
        setErr(String(e));
      }
      const name = path.split("/").pop() ?? path;
      const desc = LanguageDescription.matchFilename(languages, name);
      if (desc) {
        desc.load().then(
          (support) => view.dispatch({ effects: lang.reconfigure(support) }),
          () => {},
        );
      }
    })();
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The project watcher ticked: something under the project wrote. A clean
  // buffer follows the disk; a dirty one only raises the conflict flag when
  // this file is what moved.
  useEffect(() => {
    if (!refreshKey || !viewRef.current) return;
    (async () => {
      try {
        const f = await readTextFile(path);
        if (f.mtime_ms === mtimeRef.current) return;
        if (dirtyRef.current) {
          setConflict(true);
          return;
        }
        const view = viewRef.current;
        if (!view) return;
        mtimeRef.current = f.mtime_ms;
        setTruncated(f.truncated);
        settingRef.current = true;
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: f.content },
        });
        settingRef.current = false;
      } catch {
        /* deleted mid-view — the next explicit action will say so */
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey]);

  useEffect(() => {
    if (active) viewRef.current?.requestMeasure();
  }, [active]);

  return (
    <div className="file-view">
      <div className="file-bar">
        <span className="file-bar-path" title={path}>{homeAbbrev(path)}</span>
        {truncated && <span className="file-bar-note">first 2 MB — read-only</span>}
        <button
          className="tui-plain file-bar-btn"
          disabled={!dirty || truncated}
          title="Save (Ctrl+S)"
          onClick={() => save()}
        >
          Save{dirty ? " ●" : ""}
        </button>
        <button
          className="icon-btn"
          title="Open with the system app"
          onClick={() => openPath(path).catch(() => {})}
        >↗</button>
      </div>
      {conflict && (
        <div className="file-banner">
          <span>
            Changed on disk since you opened it — likely the agent in this
            session.
          </span>
          <button className="tui-plain" onClick={() => loadFromDisk()}>
            Reload theirs
          </button>
          <button className="tui-plain danger" onClick={() => save(true)}>
            Overwrite with mine
          </button>
        </div>
      )}
      {saveErr && <div className="file-banner error">{saveErr}</div>}
      {err ? (
        <div className="empty-note">
          Can't open {homeAbbrev(path)}: {err}
        </div>
      ) : (
        <div className="file-editor" ref={hostRef} />
      )}
    </div>
  );
}
