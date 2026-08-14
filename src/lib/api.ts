import { invoke } from "@tauri-apps/api/core";
import { LogicalSize, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { EntryPage, EntryRow, ReminderRow, SearchHit, Settings, SummaryRow, TreeStats } from "./types";

export interface WindowState {
  compactWidth?: number;
  compactHeight?: number;
  fullWidth?: number;
  fullHeight?: number;
  x?: number;
  y?: number;
}

export interface ReminderInput {
  title: string;
  note: string;
  kind: "once" | "daily" | "weekly";
  timeOfDay: string;
  weekday: number | null;
  fireDate: string | null;
}

export const win = getCurrentWindow();

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  search: (query: string, limit = 50) => invoke<SearchHit[]>("search", { query, limit }),
  addEntry: (content: string) => invoke<EntryRow>("add_entry", { content }),
  updateEntry: (id: number, title: string, category: string, tags: string) =>
    invoke<void>("update_entry", { id, title, category, tags }),
  getEntry: (id: number) => invoke<EntryRow>("get_entry", { id }),
  listSummaries: () => invoke<SummaryRow[]>("list_summaries"),
  getSummary: (date: string) => invoke<SummaryRow>("get_summary", { date }),
  getTreeStats: () => invoke<TreeStats>("get_tree_stats"),
  listEntriesFiltered: (category?: string, source?: string, query?: string, limit = 20, offset = 0) =>
    invoke<EntryPage>("list_entries_filtered", { category, source, query, limit, offset }),
  rerunSummary: (date: string) => invoke<SummaryRow>("rerun_summary", { date }),
  aiAnswer: (query: string, hits: SearchHit[]) => invoke<string>("ai_answer", { query, hits }),
  aiSearchTerms: (query: string) => invoke<string[]>("ai_search_terms", { query }),
  copyText: (text: string) => invoke<void>("copy_text", { text }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  countTodayEntries: () => invoke<number>("count_today_entries"),
  getWindowState: () => invoke<WindowState>("get_window_state"),
  saveWindowState: (state: WindowState) => invoke<void>("save_window_state", { state }),
  listReminders: () => invoke<ReminderRow[]>("list_reminders"),
  addReminder: (r: ReminderInput) =>
    invoke<ReminderRow>("add_reminder", {
      title: r.title,
      note: r.note,
      kind: r.kind,
      timeOfDay: r.timeOfDay,
      weekday: r.weekday,
      fireDate: r.fireDate,
    }),
  updateReminder: (id: number, r: ReminderInput) =>
    invoke<void>("update_reminder", {
      id,
      title: r.title,
      note: r.note,
      kind: r.kind,
      timeOfDay: r.timeOfDay,
      weekday: r.weekday,
      fireDate: r.fireDate,
    }),
  deleteReminder: (id: number) => invoke<void>("delete_reminder", { id }),
  setReminderEnabled: (id: number, enabled: boolean) =>
    invoke<void>("set_reminder_enabled", { id, enabled }),
};

// 记住的窗口状态（精简/全量各自的大小 + 共用位置），启动时加载并缓存
let winState: WindowState = {};

export async function loadWindowState() {
  winState = await api.getWindowState().catch(() => ({}));
}

export function getCachedWindowState(): WindowState {
  return winState;
}

export function setCachedWindowState(s: WindowState) {
  winState = s;
}

async function applyModeSize(mode: "compact" | "full", defW: number, defH: number) {
  const w = mode === "compact" ? winState.compactWidth : winState.fullWidth;
  const h = mode === "compact" ? winState.compactHeight : winState.fullHeight;
  if (w && h) {
    await win.setSize(new PhysicalSize(w, h));
  } else {
    await win.setSize(new LogicalSize(defW, defH));
  }
  if (winState.x != null && winState.y != null) {
    await win.setPosition(new PhysicalPosition(winState.x, winState.y));
  } else {
    await win.center();
  }
}

export async function setCompactMode() {
  await applyModeSize("compact", 760, 560);
}

export async function setFullMode() {
  await applyModeSize("full", 1080, 720);
}

export function sourceLabel(source: string): string {
  return (
    {
      clipboard: "剪贴板",
      manual: "灵感",
      summary: "汇总",
    }[source] ?? source
  );
}

export function clipText(text: string, max = 240): string {
  if (text.length <= max) return text;
  return text.slice(0, max) + "…";
}

const WEEKDAY_LABELS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

export function summaryTitle(date: string): string {
  return /-W\d{2}$/.test(date) ? `每周汇总 ${date}` : `每日汇总 ${date}`;
}

export function reminderRuleText(r: ReminderRow): string {
  if (r.kind === "daily") return `每天 ${r.timeOfDay}`;
  if (r.kind === "weekly") return `每${WEEKDAY_LABELS[r.weekday ?? 0]} ${r.timeOfDay}`;
  return `单次 ${r.fireDate ?? ""} ${r.timeOfDay}`;
}

export function reminderStatus(r: ReminderRow): string {
  if (r.enabled) return "待触发";
  if (r.kind !== "once") return "已停用";
  return r.lastFiredAt ? "已完成" : "已错过";
}

export function today(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

export function yesterday(): string {
  const d = new Date();
  d.setDate(d.getDate() - 1);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}
