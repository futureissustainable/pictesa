import type { GenerationRequest, Job } from "./types";

// Commands and configuration snippets that let agents repeat or drive a generation.

/** The address agents should use: this origin, or in development the server behind the proxy. */
export const serverOrigin = () => (import.meta.env.DEV ? __PICTESA_SERVER__ : location.origin);

export const shellQuote = (value: string) => (/^[\w@%+=:,./-]+$/.test(value) ? value : `'${value.replaceAll("'", `'\\''`)}'`);

export function requestFromJob(job: Job): GenerationRequest {
  const request: GenerationRequest = {
    model: job.model,
    prompt: job.prompt ?? "",
    negative_prompt: job.negative_prompt ?? null,
    params: job.params ?? {},
    inputs: job.inputs ?? [],
    count: job.count || 1,
  };
  if (job.preset) request.preset = job.preset;
  return request;
}

/** `pictesa generate ...` as the CLI takes it: the prompt is positional, library inputs are `role=asset:<id>`. */
export function cliCommand(request: GenerationRequest): string {
  const parts = ["pictesa generate", `--model ${shellQuote(request.model)}`];
  for (const [key, value] of Object.entries(request.params)) {
    if (value !== null) parts.push(`--param ${shellQuote(`${key}=${value}`)}`);
  }
  for (const input of request.inputs) parts.push(`--input ${shellQuote(`${input.role}=asset:${input.asset_id}`)}`);
  if (request.count > 1) parts.push(`--count ${request.count}`);
  if (request.negative_prompt) parts.push(`--negative ${shellQuote(request.negative_prompt)}`);
  if (request.preset) parts.push(`--preset ${shellQuote(request.preset)}`);
  if (request.prompt) parts.push(shellQuote(request.prompt));
  return parts.join(" \\\n  ");
}

export function curlCommand(request: GenerationRequest, hosted: boolean): string {
  const body = JSON.stringify(request, null, 2);
  const lines = [`curl -X POST ${shellQuote(`${serverOrigin()}/api/generations`)}`, `-H 'Content-Type: application/json'`];
  if (hosted) lines.push(`-H "Authorization: Bearer $PICTESA_TOKEN"`);
  lines.push(`-d ${shellQuote(body)}`);
  return lines.join(" \\\n  ");
}

export type McpClient = "claude" | "codex" | "cursor";
export type McpTransport = "stdio" | "http";

export interface McpSnippet {
  where: string;
  language: "shell" | "toml" | "json";
  code: string;
}

/** MCP setup for each agent, for the local `pictesa mcp` command or the HTTP endpoint. */
export function mcpSnippets(client: McpClient, transport: McpTransport, hosted: boolean): McpSnippet[] {
  const url = `${serverOrigin()}/mcp`;
  const token = hosted ? "pct_your_token" : null;
  if (client === "claude") {
    if (transport === "stdio") return [{ where: "Terminal", language: "shell", code: "claude mcp add pictesa -- pictesa mcp" }];
    return [
      {
        where: "Terminal",
        language: "shell",
        code: `claude mcp add --transport http pictesa ${url}${token ? ` \\\n  --header "Authorization: Bearer ${token}"` : ""}`,
      },
    ];
  }
  if (client === "codex") {
    if (transport === "stdio") {
      return [
        { where: "Terminal", language: "shell", code: "codex mcp add pictesa -- pictesa mcp" },
        { where: "Or ~/.codex/config.toml", language: "toml", code: `[mcp_servers.pictesa]\ncommand = "pictesa"\nargs = ["mcp"]` },
      ];
    }
    return [
      {
        where: "~/.codex/config.toml",
        language: "toml",
        code: `[mcp_servers.pictesa]\nurl = "${url}"${token ? `\nbearer_token_env_var = "PICTESA_TOKEN"` : ""}`,
      },
    ];
  }
  const server = transport === "stdio" ? { command: "pictesa", args: ["mcp"] } : { url, ...(token ? { headers: { Authorization: `Bearer ${token}` } } : {}) };
  return [{ where: "~/.cursor/mcp.json", language: "json", code: JSON.stringify({ mcpServers: { pictesa: server } }, null, 2) }];
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // Plain-HTTP hosts have no async clipboard; fall back to a hidden selection.
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    let ok = false;
    try {
      ok = document.execCommand("copy");
    } catch {
      ok = false;
    }
    area.remove();
    return ok;
  }
}
