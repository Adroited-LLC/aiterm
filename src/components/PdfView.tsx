/**
 * A PDF in a file tab, the way a markdown file gets a preview: every page
 * drawn in order, at the tab's width, scrolling as one document. pdf.js does
 * the drawing; the bytes come over the scoped asset protocol, the same road
 * an image or an HTML preview takes.
 *
 * Pages are laid out lazily — a 300-page manual should not render 300
 * canvases on open — with a page's canvas drawn when it scrolls near and
 * dropped when it scrolls far, so memory stays with what is on screen.
 */
import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "../platform";
import * as pdfjs from "pdfjs-dist";
import type { PDFDocumentProxy } from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { homeAbbrev } from "../ipc";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

export function isPdf(path: string): boolean {
  return /\.pdf$/i.test(path);
}

export default function PdfView({ path, active, refreshKey = 0 }: { path: string; active: boolean; refreshKey?: number }) {
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [width, setWidth] = useState(0);
  const hostRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let dead = false;
    let task: ReturnType<typeof pdfjs.getDocument> | null = null;
    setDoc(null); setErr(null);
    (async () => {
      try {
        const res = await fetch(convertFileSrc(path) + "?v=" + refreshKey);
        if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
        const data = new Uint8Array(await res.arrayBuffer());
        if (dead) return;
        task = pdfjs.getDocument({ data });
        const d = await task.promise;
        if (!dead) setDoc(d);
      } catch (e) {
        if (!dead) setErr(String(e instanceof Error ? e.message : e));
      }
    })();
    // The loading task owns the document and its worker; ending it frees both.
    return () => { dead = true; void task?.destroy(); };
  }, [path, refreshKey]);

  // The page width follows the tab; a resize re-draws at the new width.
  useEffect(() => {
    const el = hostRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(Math.max(0, el.clientWidth - 48)));
    ro.observe(el);
    setWidth(Math.max(0, el.clientWidth - 48));
    return () => ro.disconnect();
  }, []);

  return (
    <div className="pdf-view" ref={hostRef} title={homeAbbrev(path)}>
      {err && <div className="pdf-err">Could not open this PDF: {err}</div>}
      {!err && !doc && <div className="pdf-err">Opening…</div>}
      {doc && width > 0 && Array.from({ length: doc.numPages }, (_, i) => (
        <PdfPage key={i + 1} doc={doc} n={i + 1} width={width} active={active} />
      ))}
    </div>
  );
}

/** One page: a box of the page's proportions that holds its place while it
 *  is off screen, and a canvas drawn into it while it is near. */
function PdfPage({ doc, n, width, active }: { doc: PDFDocumentProxy; n: number; width: number; active: boolean }) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [ratio, setRatio] = useState(1.294); // Letter, until the page says
  const [near, setNear] = useState(n <= 2);

  useEffect(() => {
    let dead = false;
    void doc.getPage(n).then((p) => {
      if (dead) return;
      const v = p.getViewport({ scale: 1 });
      setRatio(v.height / v.width);
    });
    return () => { dead = true; };
  }, [doc, n]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const io = new IntersectionObserver(([e]) => setNear(e.isIntersecting), { rootMargin: "1200px 0px" });
    io.observe(el);
    return () => io.disconnect();
  }, []);

  useEffect(() => {
    const el = ref.current;
    if (!el || !near || !active || width <= 0) return;
    let dead = false;
    const canvas = document.createElement("canvas");
    void doc.getPage(n).then(async (p) => {
      if (dead) return;
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      const v = p.getViewport({ scale: width / p.getViewport({ scale: 1 }).width });
      canvas.width = Math.floor(v.width * dpr);
      canvas.height = Math.floor(v.height * dpr);
      canvas.style.width = `${Math.floor(v.width)}px`;
      canvas.style.height = `${Math.floor(v.height)}px`;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.scale(dpr, dpr);
      await p.render({ canvasContext: ctx, viewport: v, canvas }).promise;
      if (dead) return;
      el.replaceChildren(canvas);
    });
    return () => { dead = true; el.replaceChildren(); };
  }, [doc, n, width, near, active]);

  return <div className="pdf-page" ref={ref} style={{ width, height: Math.round(width * ratio) }} data-page={n} />;
}
