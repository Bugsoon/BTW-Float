import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, Globe } from "lucide-react";
import type { SearchSource } from "../types/app";

interface SourceListProps {
  sources: SearchSource[];
}

function extractDomain(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

export function SourceList({ sources }: SourceListProps) {
  const [expanded, setExpanded] = useState(false);

  if (!sources || sources.length === 0) return null;

  const visibleSources = expanded ? sources : sources.slice(0, 4);
  const hiddenCount = sources.length - visibleSources.length;

  return (
    <div className="mt-2.5 flex flex-wrap items-center gap-1.5">
      <span className="inline-flex items-center gap-1 text-[11px] text-slate-500">
        <Globe className="h-3 w-3" />
        参考来源:
      </span>
      {visibleSources.map((src, i) => (
        <button
          key={`${src.url}-${i}`}
          type="button"
          onClick={() => void openUrl(src.url)}
          title={`${src.title}\n${src.url}`}
          className="inline-flex max-w-[170px] items-center gap-1 rounded-full border border-slate-900/10 bg-slate-900/5 px-2 py-0.5 text-[11px] text-sky-800 transition hover:border-sky-500/40 hover:bg-sky-500/12"
        >
          <span className="truncate">{src.title || extractDomain(src.url)}</span>
          <ExternalLink className="h-2.5 w-2.5 shrink-0 opacity-70" />
        </button>
      ))}
      {hiddenCount > 0 && (
        <button
          type="button"
          onClick={() => setExpanded(true)}
          className="rounded-full bg-slate-900/6 px-2 py-0.5 text-[11px] text-slate-600 transition hover:bg-slate-900/12"
        >
          +{hiddenCount} 个
        </button>
      )}
    </div>
  );
}
