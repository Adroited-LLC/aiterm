import { useEffect, useRef, useState } from "react";

type TimeFmt = "12h" | "24h";
const FMT_KEY = "aiterm.clockFormat";

const pad = (n: number) => n.toString().padStart(2, "0");

function formatTime(d: Date, fmt: TimeFmt): string {
  const m = pad(d.getMinutes());
  const s = pad(d.getSeconds());
  if (fmt === "24h") return `${pad(d.getHours())}:${m}:${s}`;
  const ampm = d.getHours() >= 12 ? "PM" : "AM";
  const h12 = d.getHours() % 12 || 12;
  return `${h12}:${m}:${s} ${ampm}`;
}

/** Live clock to the right of the usage bars. Ticks every second; click the
 *  face to pick 12-hour (AM/PM) or 24-hour (military) format, persisted. */
export function Clock() {
  const [now, setNow] = useState<Date>(() => new Date());
  const [fmt, setFmt] = useState<TimeFmt>(
    () => (localStorage.getItem(FMT_KEY) === "24h" ? "24h" : "12h"),
  );
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const iv = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(iv);
  }, []);

  useEffect(() => {
    localStorage.setItem(FMT_KEY, fmt);
  }, [fmt]);

  // Dismiss the format menu on an outside click.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const pick = (f: TimeFmt) => {
    setFmt(f);
    setOpen(false);
  };

  return (
    <div className="clock" ref={ref}>
      <button
        className="clock-face"
        title="Click to change time format"
        onClick={() => setOpen((o) => !o)}
      >
        {formatTime(now, fmt)}
      </button>
      {open && (
        <div className="clock-menu">
          <button className={fmt === "12h" ? "on" : ""} onClick={() => pick("12h")}>
            12-hour (AM/PM)
          </button>
          <button className={fmt === "24h" ? "on" : ""} onClick={() => pick("24h")}>
            24-hour (military)
          </button>
        </div>
      )}
    </div>
  );
}
