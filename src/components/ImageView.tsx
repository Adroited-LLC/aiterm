import { useEffect, useRef, useState } from "react";
import { ExternalLink, Maximize, RotateCcw, ZoomIn, ZoomOut } from "lucide-react";
import { convertFileSrc } from "../platform";
import { homeAbbrev, openPath, fileRevision } from "../ipc";
import { fitImageScale } from "../imageFiles";
import Icon from "./Icon";

/** Images remain in an img element, including SVG: no document scripts or IPC. */
export default function ImageView({ path, active, refreshKey }: {
  path: string; active: boolean; refreshKey: number;
}) {
  const viewport = useRef<HTMLDivElement>(null);
  const [display, setDisplay] = useState<{ source: string; width: number; height: number } | null>(null);
  const loaded = useRef<{ path: string; token: string; revision: number } | null>(null);
  const size = display ?? { width: 0, height: 0 };
  const [bounds, setBounds] = useState({ width: 0, height: 0 });
  const [zoom, setZoom] = useState<number | null>(null);
  const [revision, setRevision] = useState(0);
  const [imageError, setImageError] = useState<string | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);
  const fit = fitImageScale(size.width, size.height, bounds.width - 48, bounds.height - 48);
  const scale = zoom ?? fit;
  const ready = display !== null;
  const changeZoom = (factor: number) => setZoom(Math.max(0.01, Math.min(8, scale * factor)));

  useEffect(() => {
    const el = viewport.current;
    if (!el || !active) return;
    const observer = new ResizeObserver(() => setBounds({ width: el.clientWidth, height: el.clientHeight }));
    observer.observe(el);
    setBounds({ width: el.clientWidth, height: el.clientHeight });
    return () => observer.disconnect();
  }, [active]);

  // A project notification is only a hint: unrelated writes must not replace
  // the image. Decode a changed file off-screen and retain the previous frame
  // until it is ready, including when a writer temporarily leaves a partial file.
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void (async () => {
      try {
        const token = await fileRevision(path);
        if (cancelled) return;
        const previous = loaded.current;
        if (previous?.path === path && previous.token === token && previous.revision === revision) {
          setImageError(null);
          return;
        }
        const source = convertFileSrc(path) + `?v=${encodeURIComponent(token)}&r=${revision}`;
        const next = new Image();
        next.src = source;
        await next.decode();
        if (cancelled) return;
        setDisplay({ source, width: next.naturalWidth, height: next.naturalHeight });
        loaded.current = { path, token, revision };
        setImageError(null);
      } catch (error) {
        if (!cancelled) setImageError(String(error));
      }
    })();
    return () => { cancelled = true; };
  }, [path, active, refreshKey, revision]);

  return <div className="file-view image-view">
    <div className="file-bar image-bar">
      <span className="file-bar-path" title={path}>{homeAbbrev(path)}</span>
      <div className="image-controls" role="group" aria-label="Image zoom">
        <button className="icon-btn" aria-label="Zoom out" title="Zoom out" disabled={!ready || scale <= 0.01} onClick={() => changeZoom(1 / 1.25)}><Icon of={ZoomOut} /></button>
        <span className="image-zoom">{ready ? `${Math.round(scale * 100)}%` : "—"}</span>
        <button className="icon-btn" aria-label="Zoom in" title="Zoom in" disabled={!ready || scale >= 8} onClick={() => changeZoom(1.25)}><Icon of={ZoomIn} /></button>
        <button className={"image-action" + (zoom === null ? " on" : "")} aria-pressed={zoom === null} disabled={!ready} title="Fit image to window" onClick={() => setZoom(null)}><Icon of={Maximize} size="sm" /> Fit</button>
        <button className={"image-action" + (zoom === 1 ? " on" : "")} aria-pressed={zoom === 1} disabled={!ready} title="Actual size" onClick={() => setZoom(1)}>100%</button>
      </div>
      <button className="icon-btn" aria-label="Reload image" title="Reload image" onClick={() => setRevision(n => n + 1)}><Icon of={RotateCcw} size="sm" /></button>
      <button className="icon-btn" aria-label="Open image in system app" title="Open with the system app" onClick={() => openPath(path).catch(e => setOpenError(String(e)))}><Icon of={ExternalLink} size="sm" /></button>
    </div>
    {imageError && ready && <div className="file-banner error" role="alert">Could not refresh this image. Showing the last loaded version. Use Reload to try again.</div>}
    {openError && <div className="file-banner error" role="alert">{openError}</div>}
    <div className="image-viewport" ref={viewport} tabIndex={active ? 0 : -1} role="region" aria-label="Image viewer. Use arrow keys or scroll to pan."
      onKeyDown={e => {
        if (e.target !== e.currentTarget || e.ctrlKey || e.metaKey || e.altKey || !ready) return;
        if (e.key === "+" || e.key === "=") { e.preventDefault(); changeZoom(1.25); }
        else if (e.key === "-") { e.preventDefault(); changeZoom(1 / 1.25); }
        else if (e.key === "0") { e.preventDefault(); setZoom(null); }
        else if (e.key === "1") { e.preventDefault(); setZoom(1); }
      }}>
      {!ready && imageError ? <div className="image-message" role="alert"><strong>Could not display this image</strong><span>The file may be missing, damaged, or use an unsupported image encoding.</span><button className="image-action" onClick={() => setRevision(n => n + 1)}>Try again</button></div> : <div className="image-stage">
        {!display && <span className="image-loading" role="status">Opening image…</span>}
        {display && <img src={display.source} alt={path.split(/[\\/]/).pop() ?? path} draggable={false}
          className="image-canvas" style={{ width: size.width * scale, height: size.height * scale }} />}
      </div>}
    </div>
    <div className="image-footer"><span>{ready ? `${size.width} × ${size.height}` : "Image preview"}</span><span>Scroll to pan · + / − to zoom · 0 to fit</span></div>
  </div>;
}
