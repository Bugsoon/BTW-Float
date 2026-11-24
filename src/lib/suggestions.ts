/**
 * 0ms local heuristic suggestion engine.
 * Infers likely user intents immediately from attached selection text, screenshots, or input prefix.
 */

const ERROR_OR_CODE_RE =
  /(traceback|exception|error(\[|:|\b)|panic!|warning:|failed|undefined is not|cannot read prop|at\s+[\w.$]+\s*\(|^\s*(fn|pub|const|let|var|function|class|def|import|from|export|interface|type|SELECT|INSERT|UPDATE)\b|[{}();=>]{2,})/im;

const URL_RE = /^https?:\/\/[^\s]+$/i;

function isMostlyForeignLanguage(text: string): boolean {
  let asciiAlpha = 0;
  let cjk = 0;
  const sample = text.slice(0, 600);
  for (const ch of sample) {
    const code = ch.charCodeAt(0);
    if ((code >= 65 && code <= 90) || (code >= 97 && code <= 122)) {
      asciiAlpha++;
    } else if (code >= 0x4e00 && code <= 0x9fff) {
      cjk++;
    }
  }
  return asciiAlpha >= 12 && cjk * 4 < asciiAlpha;
}

export function computeSmartSuggestions(params: {
  attachedText: string | null;
  imageCount: number;
  quickPrompts: string[];
}): string[] {
  const { attachedText, imageCount, quickPrompts } = params;

  if (imageCount > 0 && attachedText) {
    return [
      "结合截图和选中内容帮我解答",
      "排查图中的报错原因",
      "翻译并解释重点",
    ];
  }

  if (imageCount > 0) {
    return [
      "解释图里的内容或公式",
      "提取图中文字/公式 (OCR)",
      "翻译图中文字",
      "总结图表核心结论",
    ];
  }

  if (attachedText) {
    const trimmed = attachedText.trim();
    if (URL_RE.test(trimmed)) {
      return ["总结这个链接的核心内容", "提取关键信息与结论"];
    }
    if (ERROR_OR_CODE_RE.test(trimmed)) {
      return [
        "排查报错原因并给修复方案",
        "逐行解释这段代码",
        "帮我优化重构写法",
      ];
    }
    if (isMostlyForeignLanguage(trimmed)) {
      return ["翻译成地道中文", "提炼核心要点", "解释其中的专业术语"];
    }
    return ["总结成 3 条要点", "翻译成地道英文", "润色得更清晰专业"];
  }

  return quickPrompts.length > 0
    ? quickPrompts
    : ["帮我快速总结核心要点", "翻译成地道中文", "帮我解释这段内容"];
}

/**
 * Builds the automatic prompt when the user highlights unfamiliar text (e.g. in a paper/doc/IDE)
 * and clicks the mouse side button to immediately ask AI without typing.
 */
export function buildAutoSelectionPrompt(selectedText: string): {
  displayLabel: string;
  llmContent: string;
} {
  const trimmed = selectedText.trim();
  const preview =
    trimmed.length <= 42 ? trimmed : `${trimmed.slice(0, 42).trim()}…`;

  if (URL_RE.test(trimmed)) {
    return {
      displayLabel: `解读链接：${preview}`,
      llmContent: `请帮我总结并解读这个链接的核心内容：\n${trimmed}`,
    };
  }

  if (ERROR_OR_CODE_RE.test(trimmed)) {
    return {
      displayLabel: `解读代码/报错：${preview}`,
      llmContent: `请直接解释以下代码或报错的核心含义、原因与解决思路：\n\n\`\`\`\n${trimmed}\n\`\`\``,
    };
  }

  if (trimmed.length <= 48) {
    return {
      displayLabel: `解释术语：「${preview}」`,
      llmContent: `我在阅读文献/资料时遇到「${trimmed}」，请用简明易懂的中文帮我解释它的含义（若是外文术语或缩写，请先给出准确的中文译名/全称，再结合学术或专业语境讲清楚它的核心概念与常见用法）。`,
    };
  }

  if (isMostlyForeignLanguage(trimmed)) {
    return {
      displayLabel: `翻译并解读：${preview}`,
      llmContent: `我在阅读外文文献/资料时选中了下面这段话，请先给出准确流畅的中文翻译，再用通俗精炼的语言帮我解读其中的核心意思与关键概念：\n\n${trimmed}`,
    };
  }

  return {
    displayLabel: `深度解读：${preview}`,
    llmContent: `我在阅读资料时选中了下面这段内容，请用通俗清晰、条理分明的语言帮我解读它的核心意思、关键概念与逻辑要点：\n\n${trimmed}`,
  };
}

export function compressImageFileToDataUrl(
  file: Blob,
  maxEdge = 1920,
  quality = 0.85
): Promise<string> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file);
    const img = new Image();
    img.onload = () => {
      URL.revokeObjectURL(url);
      let { width, height } = img;
      if (width <= 0 || height <= 0) {
        reject(new Error("Invalid image"));
        return;
      }
      if (Math.max(width, height) > maxEdge) {
        const scale = maxEdge / Math.max(width, height);
        width = Math.round(width * scale);
        height = Math.round(height * scale);
      }
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const ctx = canvas.getContext("2d");
      if (!ctx) {
        reject(new Error("Canvas unavailable"));
        return;
      }
      ctx.drawImage(img, 0, 0, width, height);
      resolve(canvas.toDataURL("image/jpeg", quality));
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("Failed to decode image"));
    };
    img.src = url;
  });
}
