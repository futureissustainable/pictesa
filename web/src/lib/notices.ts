import { createStore } from "./store";

export interface Notice {
  id: number;
  text: string;
  tone: "info" | "error";
  action?: { label: string; run: () => void };
}

export const notices = createStore<Notice[]>([]);
let nextId = 1;
const timers = new Map<number, ReturnType<typeof setTimeout>>();

export function dismissNotice(id: number) {
  clearTimeout(timers.get(id));
  timers.delete(id);
  notices.set((list) => list.filter((notice) => notice.id !== id));
}

/** Shows a short message. Errors stay longer; at most three messages are visible. */
export function notify(text: string, options: { tone?: Notice["tone"]; action?: Notice["action"] } = {}) {
  const notice: Notice = { id: nextId++, text, tone: options.tone ?? "info", action: options.action };
  notices.set((list) => [...list.filter((item) => item.text !== text), notice].slice(-3));
  timers.set(notice.id, setTimeout(() => dismissNotice(notice.id), notice.tone === "error" ? 9000 : notice.action ? 7000 : 4500));
  return notice.id;
}

export const notifyError = (text: string) => notify(text, { tone: "error" });
