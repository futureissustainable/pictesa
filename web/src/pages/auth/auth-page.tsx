import { useEffect, useId, useState, type FormEvent } from "react";
import { api, errorMessage } from "../../lib/api";
import { Link, navigate, useLocation } from "../../lib/router";
import { loadSession, setUser, useSession } from "../../lib/session";
import { PixelMark } from "../../ui/brand";
import { Icon, Spinner } from "../../ui/icon";

/** Hosted mode only: shown while GET /api/auth/me has no user. */
export default function AuthPage() {
  const url = useLocation();
  const state = useSession();
  const open = state.status !== "ready" || state.me.signup !== false;
  const signup = open && url.pathname === "/signup";
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [reveal, setReveal] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const id = useId();

  useEffect(() => {
    document.title = `${signup ? "Sign up" : "Log in"} | Pictesa`;
    setError(null);
  }, [signup]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    if (signup && password.length < 10) {
      setError("Use at least 10 characters for the password.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const user = signup ? await api.signup({ email: email.trim(), password, name: name.trim() || undefined }) : await api.login({ email: email.trim(), password });
      if (url.pathname === "/login" || url.pathname === "/signup") navigate("/", { replace: true });
      setUser(user);
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };

  return (
    <main className="auth">
      <div className="auth-card">
        <PixelMark className="auth-mark" />
        <h1>{signup ? "Make your studio." : "Welcome back."}</h1>
        <p className="auth-lead">{signup ? "An account keeps your models, keys and library together." : "Log in to your Pictesa studio."}</p>
        {!open && url.pathname === "/signup" && (
          <p className="notice" role="status">
            Sign-up is closed on this server. Ask its operator for an account.
          </p>
        )}
        <form onSubmit={(event) => void submit(event)} noValidate>
          {signup && (
            <label className="field" htmlFor={`${id}-name`}>
              <span>
                Name <small>Optional</small>
              </span>
              <input id={`${id}-name`} type="text" autoComplete="name" autoFocus value={name} onChange={(event) => setName(event.target.value)} />
            </label>
          )}
          <label className="field" htmlFor={`${id}-email`}>
            <span>Email</span>
            <input id={`${id}-email`} type="email" autoComplete="email" autoFocus={!signup} required value={email} onChange={(event) => setEmail(event.target.value)} />
          </label>
          <div className="field">
            <label htmlFor={`${id}-password`}>Password</label>
            <div className="input-row">
              <input
                id={`${id}-password`}
                type={reveal ? "text" : "password"}
                autoComplete={signup ? "new-password" : "current-password"}
                required
                minLength={signup ? 10 : undefined}
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
              <button type="button" className="icon-button" aria-label={reveal ? "Hide password" : "Show password"} aria-pressed={reveal} onClick={() => setReveal(!reveal)}>
                <Icon name={reveal ? "visibility_off" : "visibility"} />
              </button>
            </div>
            {signup && <small>At least 10 characters.</small>}
          </div>
          {error && (
            <p className="notice error" role="alert">
              {error}
            </p>
          )}
          <button type="submit" className="primary-button" disabled={busy || !email.trim() || !password}>
            {busy ? <Spinner /> : null}
            {signup ? "Create account" : "Log in"}
            {!busy && <Icon name="arrow_forward" />}
          </button>
        </form>
        {open && (
          <p className="auth-switch">
            {signup ? "Already have an account?" : "New here?"}{" "}
            <Link to={signup ? "/login" : "/signup"}>{signup ? "Log in" : "Create an account"}</Link>
          </p>
        )}
      </div>
    </main>
  );
}

/** Local mode with PICTESA_TOKEN set: one field, then the server sets a cookie. */
export function TokenPage() {
  const [token, setToken] = useState("");
  const [reveal, setReveal] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const id = useId();

  useEffect(() => {
    document.title = "Access token | Pictesa";
  }, []);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!token.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.submitToken(token.trim());
      await loadSession();
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };

  return (
    <main className="auth">
      <div className="auth-card">
        <PixelMark className="auth-mark" />
        <h1>This studio is locked.</h1>
        <p className="auth-lead">Enter the access token this Pictesa server was started with (PICTESA_TOKEN). This browser keeps it as a cookie.</p>
        <form onSubmit={(event) => void submit(event)} noValidate>
          <div className="field">
            <label htmlFor={id}>Access token</label>
            <div className="input-row">
              <input
                id={id}
                type={reveal ? "text" : "password"}
                autoComplete="current-password"
                spellCheck={false}
                autoCapitalize="off"
                autoFocus
                value={token}
                onChange={(event) => setToken(event.target.value)}
              />
              <button type="button" className="icon-button" aria-label={reveal ? "Hide token" : "Show token"} aria-pressed={reveal} onClick={() => setReveal(!reveal)}>
                <Icon name={reveal ? "visibility_off" : "visibility"} />
              </button>
            </div>
          </div>
          {error && (
            <p className="notice error" role="alert">
              {error}
            </p>
          )}
          <button type="submit" className="primary-button" disabled={busy || !token.trim()}>
            {busy ? <Spinner /> : null}
            Unlock
            {!busy && <Icon name="arrow_forward" />}
          </button>
        </form>
      </div>
    </main>
  );
}
