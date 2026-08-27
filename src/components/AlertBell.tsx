import { useEffect, useRef, useState } from "react";

export interface Alert {
  /** Tab key, so picking one can go straight there. */
  key: number;
  title: string;
  /** What the session said, when it sent words rather than a bell. */
  message?: string;
  /** When it started waiting, for ordering and for "3m". */
  at: number;
}

function ago(ms: number): string {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.round(s / 60)}m`;
  return `${Math.round(s / 3600)}h`;
}

/** Waiting sessions, counted and listed.
 *
 *  A red dot answers "is anything waiting?" — which stops being the useful
 *  question the moment two things are. This answers "what, and which one first",
 *  and clicking an entry goes to that session, which is the only action the
 *  answer ever leads to. */
export default function AlertBell({ alerts, onGo }: {
  alerts: Alert[];
  onGo: (key: number) => void;
}) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const count = alerts.length;

  // An empty list should not sit open, staring at you, after the last session
  // was dealt with.
  useEffect(() => {
    if (count === 0) setOpen(false);
  }, [count]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  return (
    <div className="bell-wrap" ref={wrapRef}>
      <button
        className={"icon-btn bell-btn" + (count ? " lit" : "")}
        title={
          count === 0
            ? "Nothing waiting"
            : count === 1
              ? alerts[0].message ?? `${alerts[0].title} is waiting`
              : `${count} sessions waiting`
        }
        onClick={() => count && setOpen((v) => !v)}
      >
        <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" strokeWidth="1.5">
          <path d="M8 2a3.6 3.6 0 0 0-3.6 3.6c0 3-1.2 4-1.2 4h9.6s-1.2-1-1.2-4A3.6 3.6 0 0 0 8 2z" strokeLinejoin="round" />
          <path d="M6.6 12a1.6 1.6 0 0 0 2.8 0" strokeLinecap="round" />
        </svg>
        {/* The number matters more than the icon once there is more than one. */}
        {count > 0 && <span className="bell-count">{count > 9 ? "9+" : count}</span>}
      </button>
      {open && count > 0 && (
        <div className="bell-pop">
          <div className="bell-pop-head">
            {count} session{count === 1 ? "" : "s"} waiting
          </div>
          {alerts.map((a) => (
            <button
              key={a.key}
              className="bell-item"
              onClick={() => {
                onGo(a.key);
                setOpen(false);
              }}
            >
              <div className="bell-item-top">
                <span className="bell-item-title">{a.title}</span>
                <span className="bell-item-age">{ago(a.at)}</span>
              </div>
              {/* No message means it rang the bell without saying why —
                  older sessions, or a channel that sends no text. */}
              <div className="bell-item-msg">{a.message ?? "Waiting for your input"}</div>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
