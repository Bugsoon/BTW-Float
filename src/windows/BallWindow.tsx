import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Sparkles } from "lucide-react";
import { getBallEdgeState } from "../lib/commands";
import { useSystemDark } from "../lib/theme";

interface BallUiState {
  hovered?: boolean | null;
  pressed?: boolean | null;
}

interface BallEdgeState {
  edge: "none" | "left" | "right";
  cssSlide: boolean;
}

export function BallWindow() {
  const isDark = useSystemDark();
  const [edgeState, setEdgeState] = useState<BallEdgeState>({
    edge: "none",
    cssSlide: false,
  });
  const [hovered, setHovered] = useState(false);
  const [pressed, setPressed] = useState(false);

  useEffect(() => {
    void getBallEdgeState()
      .then((state) => setEdgeState(state))
      .catch(() => {});

    const unlistenPromises = [
      listen<BallEdgeState>("ball:edge-state", (ev) => {
        setEdgeState({
          edge: ev.payload.edge,
          cssSlide: Boolean(ev.payload.cssSlide),
        });
      }),
      listen<BallUiState>("ball:ui-state", (ev) => {
        if (typeof ev.payload.hovered === "boolean") {
          setHovered(ev.payload.hovered);
          if (!ev.payload.hovered) setPressed(false);
        }
        if (typeof ev.payload.pressed === "boolean") {
          setPressed(ev.payload.pressed);
        }
      }),
    ];

    return () => {
      unlistenPromises.forEach((p) => {
        void p.then((fn) => fn());
      });
    };
  }, []);

  const isPeeked = edgeState.edge !== "none" && !hovered;
  const cssOffsetX =
    isPeeked && edgeState.cssSlide
      ? edgeState.edge === "left"
        ? -51
        : 51
      : 0;
  const scale = pressed ? 0.92 : hovered ? 1.05 : 1;

  return (
    <div className="flex h-full w-full items-center justify-center overflow-hidden bg-transparent pb-1">
      <div
        className={`relative flex h-[52px] w-[52px] items-center justify-center rounded-full transition-all duration-200 ease-out ${
          isDark ? "frosted-orb-dark" : "frosted-orb-light"
        }`}
        style={{
          opacity: isPeeked ? 0.74 : 1,
          transform: `translateX(${cssOffsetX}px) scale(${scale})`,
        }}
      >
        {/* Subtle acrylic top gloss */}
        <div
          className="pointer-events-none absolute left-2.5 top-1.5 h-2.5 w-6 rounded-full"
          style={{
            background: isDark
              ? "linear-gradient(180deg, rgba(255,255,255,0.28) 0%, transparent 100%)"
              : "linear-gradient(180deg, rgba(255,255,255,0.95) 0%, transparent 100%)",
          }}
        />
        <Sparkles
          className={`relative z-10 h-[20px] w-[20px] ${
            isDark ? "text-sky-300" : "text-sky-600"
          }`}
          strokeWidth={2.1}
        />
      </div>
    </div>
  );
}
