import { useEffect, useRef, useState } from "react";
import { PreviewMsg, Session, homeAbbrev, relTime, sessionPreview } from "../ipc";

interface Props {
  session: Session;
  onResume: (s: Session) => void;
  onClose: () => void;
  /** Whether this session's engine can be resumed at all. The sidebar's ▶ is
   *  hidden when it cannot, and this is its twin — offering it here would put
   *  the same declined request behind a second button. */
  canResume: boolean;
}

/** Read-only tail of a session's conversation, shown in the terminal area
 *  when a sidebar item is selected — so you can check it's the right
 *  session before ▶ actually resumes it. */
export default function SessionPreview({ session, onResume, onClose, canResume }: Props) {
  const [msgs, setMsgs] = useState<PreviewMsg[] | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let stale = false;
    setMsgs(null);
    sessionPreview(session.id)
      .then((m) => !stale && setMsgs(m))
      .catch(() => !stale && setMsgs([]));
    return () => {
      stale = true;
    };
  }, [session.id]);

  // Latest messages matter most — keep the view pinned to the bottom.
  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [msgs]);

  return (
    <div className="preview-pane">
      <div className="preview-head">
        <div className="preview-head-text">
          <div className="preview-title">{session.title}</div>
          <div className="preview-sub">
            {homeAbbrev(session.project_path)}
            {session.branch ? `  ⎇ ${session.branch}` : ""}
            {"  ·  "}{relTime(session.last_active)}
          </div>
        </div>
        {canResume && (
          <button className="preview-resume" onClick={() => onResume(session)}>
            ▶ Resume session
          </button>
        )}
        <button className="icon-btn" title="Close preview" onClick={onClose}>✕</button>
      </div>
      <div className="preview-body" ref={bodyRef}>
        {msgs === null ? (
          <div className="empty-note">Loading conversation…</div>
        ) : msgs.length === 0 ? (
          <div className="empty-note">No conversation text found in this session</div>
        ) : (
          msgs.map((m, i) => (
            <div key={i} className={"preview-msg " + m.role}>
              <span className="preview-role">
                {m.role === "user" ? "❯" : "✳"}
              </span>
              <div className="preview-text">{m.text}</div>
            </div>
          ))
        )}
      </div>
      <div className="preview-foot">
        Preview — showing the last {msgs?.length ?? 0} messages
        {canResume ? " · ▶ reopens it in this window" : " · reopening isn't available for this one"}
      </div>
    </div>
  );
}
