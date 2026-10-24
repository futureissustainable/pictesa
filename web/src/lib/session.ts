import { api, ApiError, errorMessage, onUnauthorized } from "./api";
import { createStore, useStore } from "./store";
import type { Health, Me, User } from "./types";

export type SessionState =
  | { status: "loading" }
  | { status: "offline"; message: string }
  | { status: "ready"; me: Me; health: Health | null };

export const session = createStore<SessionState>({ status: "loading" });
export const useSession = () => useStore(session);

export async function loadSession() {
  try {
    const me = await api.me();
    const previous = session.get();
    session.set({ status: "ready", me, health: previous.status === "ready" ? previous.health : null });
    void refreshHealth();
  } catch (error) {
    session.set({
      status: "offline",
      message: error instanceof ApiError && error.status === 0 ? error.message : `Pictesa could not start: ${errorMessage(error)}`,
    });
  }
}

export async function refreshHealth() {
  try {
    const health = await api.health();
    session.set((state) => (state.status === "ready" ? { ...state, health } : state));
  } catch {
    // Health is informative only.
  }
}

export function setUser(user: User | null) {
  session.set((state) => (state.status === "ready" ? { ...state, me: { ...state.me, user } } : state));
}

export async function logout() {
  await api.logout();
  setUser(null);
}

// A 401 means the session cookie expired (hosted) or a token is now required (local):
// fall back to the login or token screen.
let rechecking = false;
onUnauthorized(() => {
  const state = session.get();
  if (state.status !== "ready") return;
  if (state.me.mode === "hosted" && state.me.user) setUser(null);
  else if (state.me.mode === "local" && !state.me.needs_token && !rechecking) {
    rechecking = true;
    void loadSession().finally(() => {
      rechecking = false;
    });
  }
});
