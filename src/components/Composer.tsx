import { useEffect, useRef, useState } from "react";
import { SessionStatus } from "../ipc";

interface Props {
  /** Key of the active terminal tab (composer state is kept per tab). */
  tabKey: number | null;
  tabTitle: string | null;
  onSend: (text: string) => void;
  /** Forward a raw control sequence (ctrl-c, esc) to the active PTY. */
  onControl: (seq: string) => void;
  shells: number;
  working: boolean;
  claudeStatus: SessionStatus | null;
  projectLabel: string | null;
  branch: string | null;
}

function permissionLabel(mode: string): { text: string; cls: string } {
  switch (mode) {
    case "bypassPermissions":
      return { text: "‣‣ bypass permissions on", cls: "st-red" };
    case "acceptEdits":
      return { text: "‣ accept edits on", cls: "st-yellow" };
    case "plan":
      return { text: "⏸ plan mode", cls: "st-cyan" };
    default:
      return { text: mode, cls: "st-dim" };
  }
}

export default function Composer({
  tabKey, tabTitle, onSend, onControl, shells, working, claudeStatus, projectLabel, branch,
}: Props) {
  const [text, setText] = useState("");
  const taRef = useRef<HTMLTextAreaElement>(null);
  const drafts = useRef<Map<number, string>>(new Map());
  const history = useRef<Map<number, string[]>>(new Map());
  const histPos = useRef<number>(-1);

  // Swap draft text when the active tab changes.
  const prevTab = useRef<number | null>(null);
  useEffect(() => {
    if (prevTab.current !== null && prevTab.current !== tabKey) {
      drafts.current.set(prevTab.current, text);
    }
    if (tabKey !== null && prevTab.current !== tabKey) {
      setText(drafts.current.get(tabKey) ?? "");
      histPos.current = -1;
    }
    prevTab.current = tabKey;
    taRef.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabKey]);

  const send = () => {
    if (tabKey === null) return;
    const t = text;
    if (t.trim().length === 0) {
      // Empty Enter still goes through (e.g. confirm a prompt).
      onControl("\r");
      return;
    }
    const h = history.current.get(tabKey) ?? [];
    h.push(t);
    history.current.set(tabKey, h);
    histPos.current = -1;
    setText("");
    onSend(t);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
      return;
    }
    if (e.key === "c" && e.ctrlKey) {
      const ta = taRef.current;
      const hasSelection = ta && ta.selectionStart !== ta.selectionEnd;
      if (!hasSelection) {
        e.preventDefault();
        onControl("\x03");
      }
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      onControl("\x1b");
      return;
    }
    if (e.key === "ArrowUp" && tabKey !== null) {
      const ta = taRef.current;
      const beforeCaret = text.slice(0, ta?.selectionStart ?? 0);
      if (!beforeCaret.includes("\n")) {
        const h = history.current.get(tabKey) ?? [];
        if (h.length > 0) {
          e.preventDefault();
          histPos.current = histPos.current === -1
            ? h.length - 1
            : Math.max(0, histPos.current - 1);
          setText(h[histPos.current]);
        }
      }
      return;
    }
    if (e.key === "ArrowDown" && tabKey !== null && histPos.current !== -1) {
      const h = history.current.get(tabKey) ?? [];
      e.preventDefault();
      if (histPos.current >= h.length - 1) {
        histPos.current = -1;
        setText("");
      } else {
        histPos.current += 1;
        setText(h[histPos.current]);
      }
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      const ta = taRef.current;
      if (ta) {
        const s = ta.selectionStart;
        setText(text.slice(0, s) + "  " + text.slice(ta.selectionEnd));
        requestAnimationFrame(() => ta.setSelectionRange(s + 2, s + 2));
      }
    }
  };

  const rows = Math.min(8, Math.max(1, text.split("\n").length));
  const perm = claudeStatus?.permission_mode ? permissionLabel(claudeStatus.permission_mode) : null;

  return (
    <div className="composer-wrap">
      <div className="composer">
        {tabTitle && <span className="composer-chip">{tabTitle}</span>}
        <span className="composer-prompt">❯</span>
        <textarea
          ref={taRef}
          rows={rows}
          value={text}
          spellCheck={false}
          placeholder={tabKey === null ? "No terminal open" : "Type a command or prompt — Enter to send, Shift+Enter for newline"}
          disabled={tabKey === null}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
        />
      </div>
      <div className="statusbar">
        {perm && <span className={perm.cls}>{perm.text}</span>}
        <span className="st-cyan">{shells} shell{shells === 1 ? "" : "s"}</span>
        {working && <span className="st-working">✳ working…</span>}
        <span className="st-dim">shift+enter newline · ctrl+c interrupt · esc stop</span>
        <span className="st-right st-dim">
          {projectLabel}{branch ? `  ⎇ ${branch}` : ""}
        </span>
      </div>
    </div>
  );
}
