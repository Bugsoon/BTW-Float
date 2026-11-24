import { useEffect, useRef, useState } from "react";
import {
  cancelRegionCapture,
  finishRegionCapture,
  getFrozenCapturePreview,
} from "../lib/commands";

interface DragBox {
  startX: number;
  startY: number;
  curX: number;
  curY: number;
}

export function CaptureWindow() {
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [box, setBox] = useState<DragBox | null>(null);
  const draggingRef = useRef(false);

  useEffect(() => {
    void getFrozenCapturePreview().then(setPreviewUrl);

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        void cancelRegionCapture();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const handleMouseDown = (e: React.MouseEvent<HTMLDivElement>) => {
    if (e.button === 2) {
      e.preventDefault();
      void cancelRegionCapture();
      return;
    }
    if (e.button !== 0) return;
    draggingRef.current = true;
    setBox({
      startX: e.clientX,
      startY: e.clientY,
      curX: e.clientX,
      curY: e.clientY,
    });
  };

  const handleMouseMove = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    setBox((prev) =>
      prev ? { ...prev, curX: e.clientX, curY: e.clientY } : null
    );
  };

  const handleMouseUp = (e: React.MouseEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !draggingRef.current) return;
    draggingRef.current = false;
    if (!box) return;

    const left = Math.min(box.startX, e.clientX);
    const top = Math.min(box.startY, e.clientY);
    const width = Math.abs(e.clientX - box.startX);
    const height = Math.abs(e.clientY - box.startY);

    if (width < 8 || height < 8) {
      setBox(null);
      return;
    }

    const vw = Math.max(1, window.innerWidth);
    const vh = Math.max(1, window.innerHeight);
    void finishRegionCapture(left / vw, top / vh, width / vw, height / vh);
  };

  const rect = box
    ? {
        left: Math.min(box.startX, box.curX),
        top: Math.min(box.startY, box.curY),
        width: Math.abs(box.curX - box.startX),
        height: Math.abs(box.curY - box.startY),
      }
    : null;

  return (
    <div
      className="relative h-screen w-screen cursor-crosshair overflow-hidden bg-black select-none"
      onMouseDown={handleMouseDown}
      onMouseMove={handleMouseMove}
      onMouseUp={handleMouseUp}
      onContextMenu={(e) => {
        e.preventDefault();
        void cancelRegionCapture();
      }}
    >
      {/* Frozen screen base layer */}
      {previewUrl && (
        <img
          src={previewUrl}
          alt=""
          draggable={false}
          className="pointer-events-none absolute inset-0 h-full w-full object-fill"
        />
      )}

      {/* Dim overlay when not actively selecting */}
      {!rect && (
        <div className="pointer-events-none absolute inset-0 bg-black/32" />
      )}

      {/* Top instruction pill */}
      {!rect && (
        <div className="pointer-events-none absolute left-1/2 top-6 -translate-x-1/2 rounded-full border border-white/20 bg-slate-950/75 px-4 py-1.5 text-xs text-slate-100 shadow-lg">
          按住左键拖拽框选提问区域 · 按 Esc 或右键取消
        </div>
      )}

      {/* Active selection cutout with GPU-accelerated outer dim shadow */}
      {rect && (
        <div
          className="pointer-events-none absolute border-[1.5px] border-sky-400"
          style={{
            left: `${rect.left}px`,
            top: `${rect.top}px`,
            width: `${rect.width}px`,
            height: `${rect.height}px`,
            boxShadow: "0 0 0 9999px rgba(0, 0, 0, 0.45)",
          }}
        >
          {/* Size badge */}
          <div
            className="absolute left-0 rounded-md bg-slate-950/85 px-2 py-0.5 font-mono text-[11px] text-sky-200"
            style={{
              top: rect.top > 28 ? "-24px" : "4px",
            }}
          >
            {Math.round(rect.width * (window.devicePixelRatio || 1))} ×{" "}
            {Math.round(rect.height * (window.devicePixelRatio || 1))}
          </div>
        </div>
      )}
    </div>
  );
}
