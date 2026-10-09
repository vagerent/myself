export interface Settings {
  apiKey: string;
  baseUrl: string;
  model: string;
  summaryTime: string;
  idleThresholdMin: number;
  markdownDir: string;
  promptTemplate: string;
  hotkey: string;
  autostart: boolean;
  aiAnswerEnabled: boolean;
  pollIntervalMs: number;
  theme: string;
  weeklySummaryDay: number;
  weeklySummaryTime: string;
  weeklyPromptTemplate: string;
  dedupScope: string;
  meetingPromptTemplate: string;
  meetingDeepPromptTemplate: string;
  clipboardEnabled: boolean;
  pnoteEnabled: boolean;
  pnoteKeepDays: number;
  pnoteFfmpegPath: string;
  pnoteSyncWindowDays: number;
  recorderEventPrompt: string;
  recorderMinutesPrompt: string;
  recorderDeepPrompt: string;
  recorderAnalysisIntervalHours: number;
}

export interface PnoteStatus {
  running: boolean;
  connected: boolean;
  battery: number | null;
  charging: boolean;
  capacity: [number, number] | null;
  version: string | null;
  sleeping: boolean;
  pending: number;
}

export interface EntryRow {
  id: number;
  rawContent: string;
  cleanContent: string | null;
  deepAnalysis: string | null;
  title: string | null;
  category: string | null;
  tags: string | null;
  source: string;
  processed: boolean;
  createdAt: string;
  processedAt: string | null;
}

export interface EntryPage {
  entries: EntryRow[];
  total: number;
}

export interface SummaryRow {
  date: string;
  content: string;
  markdownPath: string;
  status: string;
  createdAt: string;
}

export interface SearchHit {
  docType: string;
  docId: string;
  title: string;
  content: string;
  category: string;
  tags: string;
  source: string;
  createdAt: string;
  rank: number;
}

export interface CountRow {
  key: string;
  count: number;
}

export interface TreeStats {
  totalEntries: number;
  byCategory: CountRow[];
  bySource: CountRow[];
  summaryDates: string[];
}

export interface ReminderRow {
  id: number;
  title: string;
  note: string;
  kind: "once" | "daily" | "weekly";
  timeOfDay: string;
  weekday: number | null;
  fireDate: string | null;
  nextFireAt: string;
  enabled: boolean;
  lastFiredAt: string | null;
  createdAt: string;
}

export const CATEGORIES = ["灵感", "代码", "文章", "链接", "待办", "会议", "其他"] as const;

export const SOURCE_LABEL: Record<string, string> = {
  clipboard: "剪贴板",
  manual: "灵感",
  summary: "汇总",
  meeting: "会议录音",
  recorder: "录音卡",
};

export interface AsrModelStatus {
  downloaded: boolean;
  path: string;
  sidecarReady: boolean;
}

export interface InsightsRun {
  id: number;
  startedAt: string;
  trigger: string;
  analyzedCount: number;
  status: "ok" | "failed";
  error: string | null;
  findingsCount: number;
}

export interface InsightsFinding {
  id: number;
  runId: number;
  kind: string;
  title: string;
  note: string;
  quote: string;
  entryId: number | null;
  eventDate: string | null;
  status: "new" | "done" | "dismissed";
  reminderId: number | null;
  createdAt: string;
}

export interface InsightsToEntryResult {
  entryId: number | null;
  message?: string;
}

// 项目脉络 state（insights_threads 原样返回的 JSON，字段为 snake_case）
export interface ThreadItem {
  date: string;
  note: string;
}

export interface InsightsOpenItem {
  item: string;
  origin_date: string | null;
  last_mention: string | null;
  reported: boolean;
}

export interface ProjectThread {
  name: string;
  thread: ThreadItem[];
  open_items: InsightsOpenItem[];
}

export interface InsightsThreadsState {
  projects: ProjectThread[];
  archived: ProjectThread[];
}
