import { writable } from "svelte/store";
import { api } from "./api";
import { applyTheme } from "./theme";
import type { Settings } from "./types";

export type Mode = "compact" | "full";
export type View = "search" | "browse" | "settings" | "about";

export const mode = writable<Mode>("compact");
export const view = writable<View>("search");
export const settings = writable<Settings | null>(null);
export const toast = writable<string | null>(null);
/** 托盘「任务提醒」进入时置 true，BrowseView 消费后置 false。 */
export const pendingReminderFocus = writable(false);

let toastTimer: ReturnType<typeof setTimeout> | null = null;

export function showToast(msg: string) {
  toast.set(msg);
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.set(null), 2600);
}

export async function loadSettings() {
  try {
    const s = await api.getSettings();
    settings.set(s);
    applyTheme(s.theme);
  } catch (e) {
    applyTheme("dark");
    showToast("读取设置失败：" + String(e));
  }
}
