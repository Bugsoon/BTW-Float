import { useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, Copy } from "lucide-react";

interface MarkdownViewProps {
  content: string;
  isDark: boolean;
}

function CodeBlock({
  code,
  lang,
  isDark,
}: {
  code: string;
  lang: string;
  isDark: boolean;
}) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    } catch {
      // ignore clipboard errors
    }
  };

  return (
    <div
      className={`my-2 overflow-hidden rounded-xl border ${
        isDark
          ? "border-white/12 bg-black/35"
          : "border-slate-900/12 bg-slate-900/8"
      }`}
    >
      <div
        className={`flex items-center justify-between px-3 py-1 text-[11px] ${
          isDark
            ? "border-b border-white/10 text-slate-300/80"
            : "border-b border-slate-900/10 text-slate-600"
        }`}
      >
        <span className="font-mono uppercase tracking-wider">
          {lang || "code"}
        </span>
        <button
          type="button"
          onClick={handleCopy}
          className={`flex items-center gap-1 rounded-md px-1.5 py-0.5 transition ${
            isDark
              ? "hover:bg-white/10 text-slate-200"
              : "hover:bg-slate-900/10 text-slate-700"
          }`}
        >
          {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
          <span>{copied ? "已复制" : "复制"}</span>
        </button>
      </div>
      <pre className="glass-scroll overflow-x-auto p-3 text-[12.5px] leading-relaxed font-mono select-text">
        <code>{code}</code>
      </pre>
    </div>
  );
}

export function MarkdownView({ content, isDark }: MarkdownViewProps) {
  return (
    <div
      className={`select-text text-[13.5px] leading-relaxed break-words ${
        isDark ? "text-slate-100" : "text-slate-900"
      }`}
    >
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          p: ({ children }) => <p className="mb-2 last:mb-0">{children}</p>,
          ul: ({ children }) => (
            <ul className="mb-2 list-disc space-y-1 pl-5 last:mb-0">
              {children}
            </ul>
          ),
          ol: ({ children }) => (
            <ol className="mb-2 list-decimal space-y-1 pl-5 last:mb-0">
              {children}
            </ol>
          ),
          li: ({ children }) => <li className="leading-relaxed">{children}</li>,
          h1: ({ children }) => (
            <h1 className="mb-2 mt-3 text-base font-semibold first:mt-0">
              {children}
            </h1>
          ),
          h2: ({ children }) => (
            <h2 className="mb-1.5 mt-2.5 text-[14.5px] font-semibold first:mt-0">
              {children}
            </h2>
          ),
          h3: ({ children }) => (
            <h3 className="mb-1 mt-2 text-[13.5px] font-semibold first:mt-0">
              {children}
            </h3>
          ),
          blockquote: ({ children }) => (
            <blockquote
              className={`my-2 border-l-2 pl-3 italic ${
                isDark
                  ? "border-sky-300/60 text-slate-300"
                  : "border-sky-600/60 text-slate-700"
              }`}
            >
              {children}
            </blockquote>
          ),
          a: ({ href, children }) => (
            <a
              href={href}
              onClick={(e) => {
                e.preventDefault();
                if (href) void openUrl(href);
              }}
              className={`underline decoration-1 underline-offset-2 ${
                isDark
                  ? "text-sky-300 hover:text-sky-200"
                  : "text-sky-700 hover:text-sky-600"
              }`}
            >
              {children}
            </a>
          ),
          code: ({ className, children }) => {
            const raw = String(children ?? "").replace(/\n$/, "");
            const match = /language-([\w-]+)/.exec(className || "");
            const isBlock = Boolean(match) || raw.includes("\n");
            if (!isBlock) {
              return (
                <code
                  className={`rounded-md px-1.5 py-0.5 font-mono text-[12px] ${
                    isDark
                      ? "bg-white/14 text-sky-200"
                      : "bg-slate-900/10 text-sky-800"
                  }`}
                >
                  {raw}
                </code>
              );
            }
            return (
              <CodeBlock
                code={raw}
                lang={match?.[1] ?? ""}
                isDark={isDark}
              />
            );
          },
          pre: ({ children }) => <>{children}</>,
          table: ({ children }) => (
            <div className="glass-scroll my-2 overflow-x-auto">
              <table
                className={`w-full border-collapse text-left text-[12.5px] ${
                  isDark ? "border-white/15" : "border-slate-900/15"
                }`}
              >
                {children}
              </table>
            </div>
          ),
          th: ({ children }) => (
            <th
              className={`border px-2 py-1 font-medium ${
                isDark
                  ? "border-white/15 bg-white/8"
                  : "border-slate-900/15 bg-slate-900/6"
              }`}
            >
              {children}
            </th>
          ),
          td: ({ children }) => (
            <td
              className={`border px-2 py-1 ${
                isDark ? "border-white/12" : "border-slate-900/12"
              }`}
            >
              {children}
            </td>
          ),
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
