import { useEffect, useState, type FormEvent } from "react";
import { api, errorMessage } from "../../lib/api";
import { formatDate } from "../../lib/format";
import { useSession } from "../../lib/session";
import { mcpSnippets, serverOrigin, type McpClient, type McpTransport } from "../../lib/snippets";
import type { ApiToken } from "../../lib/types";
import { CodeBlock, CopyButton } from "../../ui/code";
import { Icon, Spinner } from "../../ui/icon";

const clients: { id: McpClient; label: string }[] = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
  { id: "cursor", label: "Cursor" },
];

const tools = ["list_models", "list_presets", "generate", "get_job", "wait_for_job", "cancel_job", "list_assets", "get_asset", "import_asset"];

export default function Agents() {
  const session = useSession();
  const hosted = session.status === "ready" && session.me.mode === "hosted";
  const [client, setClient] = useState<McpClient>("claude");
  const [transport, setTransport] = useState<McpTransport>(hosted ? "http" : "stdio");
  const active = hosted ? "http" : transport;
  const label = clients.find((item) => item.id === client)?.label ?? client;

  return (
    <div className="settings-list">
      <section className="settings-block" aria-labelledby="mcp-title">
        <h2 id="mcp-title">Connect an agent</h2>
        <p>Agents use the Model Context Protocol to list models, generate, wait for results and read the library.</p>
        <div className="agent-switches">
          <div className="segmented" role="group" aria-label="Agent">
            {clients.map((item) => (
              <button key={item.id} type="button" aria-pressed={client === item.id} onClick={() => setClient(item.id)}>
                {item.label}
              </button>
            ))}
          </div>
          {!hosted && (
            <div className="segmented" role="group" aria-label="Connection">
              <button type="button" aria-pressed={transport === "stdio"} onClick={() => setTransport("stdio")}>
                <Icon name="terminal" />
                Local command
              </button>
              <button type="button" aria-pressed={transport === "http"} onClick={() => setTransport("http")}>
                <Icon name="dns" />
                HTTP
              </button>
            </div>
          )}
        </div>
        {mcpSnippets(client, active, hosted).map((snippet) => (
          <CodeBlock key={snippet.where} code={snippet.code} label={`${label} setup`} caption={snippet.where} />
        ))}
        <p className="hint">
          {active === "stdio"
            ? "Runs pictesa mcp on this computer. The pictesa command must be on your PATH."
            : hosted
              ? `Uses ${serverOrigin()}/mcp with a personal token from below.`
              : `Uses ${serverOrigin()}/mcp while the server runs. When PICTESA_TOKEN is set, send it as a bearer token.`}
        </p>
        <div className="tool-list" aria-label="Tools agents get">
          {tools.map((tool) => (
            <code key={tool}>{tool}</code>
          ))}
        </div>
      </section>
      {hosted && <Tokens />}
    </div>
  );
}

function Tokens() {
  const [tokens, setTokens] = useState<ApiToken[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<{ name: string; token: string } | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);

  const load = () =>
    api
      .tokens()
      .then(({ tokens: list }) => setTokens(list))
      .catch((reason: unknown) => setError(errorMessage(reason)));

  useEffect(() => {
    void load();
  }, []);

  const create = async (event: FormEvent) => {
    event.preventDefault();
    if (!name.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const token = await api.createToken(name.trim());
      setCreated({ name: token.name, token: token.token });
      setName("");
      await load();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (token: ApiToken) => {
    if (confirming !== token.id) {
      setConfirming(token.id);
      return;
    }
    setConfirming(null);
    try {
      await api.deleteToken(token.id);
      setTokens((list) => list?.filter((item) => item.id !== token.id) ?? null);
    } catch (reason) {
      setError(errorMessage(reason));
    }
  };

  return (
    <section className="settings-block" aria-labelledby="tokens-title">
      <h2 id="tokens-title">API tokens</h2>
      <p>Agents and scripts send a token as Authorization: Bearer. Each token acts as you.</p>
      {created && (
        <div className="token-created" role="status">
          <p>
            <strong>{created.name}</strong> is ready. Copy it now: it is shown only once.
          </p>
          <div className="code-body">
            <pre tabIndex={0} aria-label="New token">
              <code>{created.token}</code>
            </pre>
            <CopyButton text={created.token} label="Copy token" />
          </div>
        </div>
      )}
      <form className="token-form" onSubmit={(event) => void create(event)}>
        <label className="field">
          <span className="sr-only">Token name</span>
          <input type="text" value={name} maxLength={80} placeholder="Name, for example Claude Code on laptop" onChange={(event) => setName(event.target.value)} />
        </label>
        <button type="submit" className="primary-button" disabled={!name.trim() || busy}>
          {busy ? <Spinner /> : <Icon name="key" />}
          Create token
        </button>
      </form>
      {error && (
        <p className="notice error" role="alert">
          {error}
        </p>
      )}
      {tokens === null ? (
        !error && (
          <p className="hint" role="status">
            <Spinner /> Loading tokens
          </p>
        )
      ) : tokens.length ? (
        <ul className="token-list">
          {tokens.map((token) => (
            <li key={token.id} className="connection-row">
              <span>
                {token.name}
                <small className="ledger-date">
                  Created {formatDate(token.created_at)}, {token.last_used_at ? `last used ${formatDate(token.last_used_at)}` : "never used"}
                </small>
              </span>
              <button type="button" className="text-button destructive" onClick={() => void remove(token)} onBlur={() => setConfirming(null)}>
                <Icon name="delete" />
                {confirming === token.id ? "Confirm" : "Revoke"}
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="hint">No tokens yet.</p>
      )}
    </section>
  );
}
