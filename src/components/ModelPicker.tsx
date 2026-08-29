/**
 * A model chooser for a long catalogue: the current pick with its logo, and
 * on click a popover with a search box over the whole list — every model
 * the provider serves, flat, in the provider's order, each with its vendor's
 * mark. A native select cannot search or draw a logo per row.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import BrandIcon from "./BrandIcon";
import Icon from "./Icon";
import { brandForModel } from "../brand";
import { ChevronDown, Search } from "lucide-react";

export default function ModelPicker({ value, models, onPick, placeholder, loading }: {
  value: string;
  models: string[];
  onPick: (id: string) => void;
  placeholder?: string;
  loading?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const wrap = useRef<HTMLDivElement>(null);
  const list = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) return models;
    const words = needle.split(/\s+/);
    return models.filter((m) => { const l = m.toLowerCase(); return words.every((w) => l.includes(w)); });
  }, [models, q]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => { if (!wrap.current?.contains(e.target as Node)) setOpen(false); };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", key);
    return () => { document.removeEventListener("mousedown", away); document.removeEventListener("keydown", key); };
  }, [open]);

  const pick = (id: string) => { onPick(id); setOpen(false); setQ(""); };

  return (
    <div className="mp" ref={wrap}>
      <button className="mp-current" onClick={() => setOpen((v) => !v)} title={value || undefined}>
        <BrandIcon name={brandForModel(value)} size={14} className="inline" />
        <span className="mp-current-id">{value || (loading ? "loading the catalogue…" : placeholder ?? "Choose a model")}</span>
        <Icon of={ChevronDown} size="sm" />
      </button>
      {open && (
        <div className="mp-pop">
          <div className="mp-search">
            <Icon of={Search} size="sm" />
            <input
              autoFocus
              value={q}
              placeholder={`Search ${models.length} models`}
              spellCheck={false}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && list[0]) pick(list[0]); }}
            />
          </div>
          <div className="mp-list">
            {list.map((m) => (
              <button key={m} className={"mp-row" + (m === value ? " on" : "")} onClick={() => pick(m)} title={m}>
                <BrandIcon name={brandForModel(m)} size={14} className="inline" />
                <span className="mp-row-id">{m}</span>
              </button>
            ))}
            {list.length === 0 && <div className="mp-none">{loading ? "Loading…" : "Nothing matches"}</div>}
          </div>
        </div>
      )}
    </div>
  );
}
