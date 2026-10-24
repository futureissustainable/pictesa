import { useState } from "react";
import { copyText } from "../lib/snippets";
import { Icon } from "./icon";

export function CopyButton({ text, label = "Copy", className = "" }: { text: string; label?: string; className?: string }) {
  const [state, setState] = useState<"idle" | "done" | "failed">("idle");
  return (
    <button
      type="button"
      className={`icon-button ${className}`}
      aria-label={state === "done" ? "Copied" : state === "failed" ? "Copy failed" : label}
      title={state === "done" ? "Copied" : label}
      onClick={async () => {
        setState((await copyText(text)) ? "done" : "failed");
        setTimeout(() => setState("idle"), 1600);
      }}
    >
      <Icon name={state === "done" ? "check" : "content_copy"} />
    </button>
  );
}

/** Read-only code with a copy button. */
export function CodeBlock({ code, label, caption }: { code: string; label: string; caption?: string }) {
  return (
    <figure className="code-block">
      {caption && <figcaption>{caption}</figcaption>}
      <div className="code-body">
        <pre tabIndex={0} aria-label={label}>
          <code>{code}</code>
        </pre>
        <CopyButton text={code} label={`Copy ${label}`} />
      </div>
    </figure>
  );
}
