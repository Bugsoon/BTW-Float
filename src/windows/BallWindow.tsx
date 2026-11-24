import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Sparkles } from "lucide-react";
import { useSystemDark } from "../lib/theme";

interface BallUiState {
  hovered?: boolean | null;
  pressed?: boolean | null;
}

export function BallWindow() {
  const isDark = useSystemDark();
  const [edge, setEdge] = useState<"none" | "left" | "right">("none");
  const [hovered, setHovered] = useState(false);
  const [pressed, setPressed] = useState(false);

  useEffect(() => {
    const unlistenPromises = [
      listen<{ edge: "none" | "left" | "right" }>("ball:edge-state", (ev) => {
        setEdge(ev.payload.edge);
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

  const isPeeked = edge !== "none" && !hovered;

  return (
    <div className="flex h-full w-full items-center justify-center overflow-hidden bg-transparent pb-1">
      <div
        className={`relative flex h-[52px] w-[52px] items-center justify-center rounded-full transition-all duration-150 ${
          isDark ? "frosted-orb-dark" : "frosted-orb-light"
        }`}
        style={{
          opacity: isPeeked ? 0.68 : 1,
          transform: pressed
            ? "scale(0.92)"
            : hovered
            ? "scale(1.05)"
            : "scale(1)",
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
