<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { api } from "../lib/api";
  import { showToast } from "../lib/stores";
  import type { InsightsFinding, InsightsRun, InsightsThreadsState, ProjectThread } from "../lib/types";

  export let onOpenEntry: (entryId: number) => void;
  export let onUnreadChange: () => void = () => {};

  let tab: "findings" | "threads" = "findings";
  let busy = false;
  let loading = true;
  let runs: InsightsRun[] = [];
  let findingsByRun: Map<number, InsightsFinding[]> = new Map();
  let threads: InsightsThreadsState | null = null;
  let expandedQuotes: Record<number, boolean> = {};
  let showArchived = false;

  const KIND_ICON: Record<string, string> = {
    commitment_mine: "🙋",
    commitment_other: "🤝",
    idea: "💡",
    open_loop: "🔁",
  };
  const KIND_LABEL: Record<string, string> = {
    commitment_mine: "我要做的 ⭐",
    commitment_other: "别人答应的",
    idea: "灵感",
    open_loop: "未闭环 ⭐",
  };

  function kindIcon(kind: string): string {
    return KIND_ICON[kind] ?? "📌";
  }

  function kindLabel(kind: string): string {
    return KIND_LABEL[kind] ?? kind;
  }

  function daysAgo(date: string): number | null {
    const d = new Date(date + "T00:00:00");
    if (isNaN(d.getTime())) return null;
    const now = new Date();
    const today0 = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    return Math.max(0, Math.round((today0.getTime() - d.getTime()) / 86400000));
  }

  function sortedThread(p: ProjectThread) {
    return [...p.thread].sort((a, b) => a.date.localeCompare(b.date));
  }

  async function refreshFindings() {
    try {
      const rs = await api.insightsRuns();
      rs.sort((a, b) => b.id - a.id);
      runs = rs;
      const pairs = await Promise.all(rs.map(async (r) => [r.id, await api.insightsFindings(r.id)] as const));
      findingsByRun = new Map(pairs);
    } catch (e) {
      showToast("读取洞察失败：" + String(e));
    } finally {
      loading = false;
    }
  }

  async function refreshThreads() {
    try {
      threads = await api.insightsThreads();
    } catch (e) {
      showToast("读取脉络失败：" + String(e));
    }
  }

  function markRead() {
    api.insightsMarkRead().then(onUnreadChange).catch(() => {});
  }

  async function analyzeNow() {
    if (busy) return;
    busy = true;
    try {
      await api.insightsAnalyzeNow();
      showToast("分析完成");
    } catch (e) {
      showToast(String(e));
    } finally {
      busy = false;
    }
  }

  function toggleQuote(id: number) {
    expandedQuotes = { ...expandedQuotes, [id]: !expandedQuotes[id] };
  }

  async function toReminder(f: InsightsFinding) {
    try {
      await api.insightsToReminder(f.id);
      showToast("已创建提醒");
      refreshFindings();
    } catch (e) {
      showToast(String(e));
    }
  }

  async function toEntry(f: InsightsFinding) {
    try {
      const res = await api.insightsToEntry(f.id);
      showToast(res.message ?? "已转为条目");
      refreshFindings();
    } catch (e) {
      showToast(String(e));
    }
  }

  async function dismiss(f: InsightsFinding) {
    try {
      await api.insightsDismiss(f.id);
      refreshFindings();
    } catch (e) {
      showToast(String(e));
    }
  }

  onMount(() => {
    markRead();
    refreshFindings();
    refreshThreads();
    const unlisten = listen("insights-updated", () => {
      refreshFindings();
      refreshThreads();
      markRead();
    });
    return () => {
      unlisten.then((f) => f());
    };
  });
</script>

<div class="insights-head">
  <strong>录音洞察</strong>
  <button class="primary" on:click={analyzeNow} disabled={busy}>
    {busy ? "分析中…" : "立即分析"}
  </button>
  {#if busy}
    <span class="spinner"></span>
  {/if}
  <span style="flex:1;"></span>
  <div class="detail-tabs">
    <button class:active={tab === "findings"} on:click={() => (tab = "findings")}>发现</button>
    <button class:active={tab === "threads"} on:click={() => (tab = "threads")}>脉络</button>
  </div>
</div>

<div class="results" style="padding:10px;">
  {#if tab === "findings"}
    {#if loading}
      <div class="loading"><span class="spinner"></span>加载中…</div>
    {:else if runs.length === 0}
      <div class="empty">还没有分析记录，点「立即分析」开始第一次洞察分析</div>
    {:else}
      {#each runs as run (run.id)}
        <div class="run-group">
          <div class="run-head">
            <span class="run-time">{run.startedAt.slice(0, 16)}</span>
            <span class="badge">{run.trigger === "manual" ? "手动" : "定时"}</span>
            <span class="hint">分析 {run.analyzedCount} 条 · 发现 {run.findingsCount} 条</span>
          </div>
          {#if run.status === "failed"}
            <div class="run-error">分析失败：{run.error ?? "未知错误"}</div>
          {:else}
            {#each findingsByRun.get(run.id) ?? [] as f (f.id)}
              <div class="finding" class:dismissed={f.status === "dismissed"}>
                <div class="finding-title">
                  <span class="badge">{kindIcon(f.kind)} {kindLabel(f.kind)}</span>
                  <strong>{f.title}</strong>
                  {#if f.eventDate}
                    <span class="badge">{f.eventDate}</span>
                  {/if}
                </div>
                {#if f.note}
                  <div class="finding-note">{f.note}</div>
                {/if}
                {#if f.quote}
                  {#if expandedQuotes[f.id]}
                    <blockquote class="finding-quote">{f.quote}</blockquote>
                    <button class="icon-btn" on:click={() => toggleQuote(f.id)}>收起引文</button>
                  {:else}
                    <button class="icon-btn" on:click={() => toggleQuote(f.id)}>查看原文引文</button>
                  {/if}
                {/if}
                {#if f.status !== "dismissed"}
                  <div class="finding-actions">
                    {#if f.entryId != null}
                      <button class="sm" on:click={() => onOpenEntry(f.entryId)}>来源条目 #{f.entryId}</button>
                    {/if}
                    <span style="flex:1;"></span>
                    {#if f.reminderId != null}
                      <button disabled>已创建提醒</button>
                    {:else}
                      <button on:click={() => toReminder(f)}>转提醒</button>
                      <button on:click={() => toEntry(f)}>转成条目</button>
                      <button on:click={() => dismiss(f)}>忽略</button>
                    {/if}
                  </div>
                {/if}
              </div>
            {:else}
              <div class="empty-run">本次没有发现值得提醒的事</div>
            {/each}
          {/if}
        </div>
      {/each}
    {/if}
  {:else}
    {#if !threads || (threads.projects.length === 0 && threads.archived.length === 0)}
      <div class="empty">还没有项目脉络，完成几次洞察分析后，这里会生长出跨天时间线</div>
    {:else}
      {#each threads.projects as p (p.name)}
        <div class="project-card">
          <div class="project-name">{p.name}</div>
          {#if p.thread.length > 0}
            <div class="thread">
              {#each sortedThread(p) as t}
                <div class="thread-item">
                  <span class="badge">{t.date}</span>
                  <span class="thread-note">{t.note}</span>
                </div>
              {/each}
            </div>
          {/if}
          {#if p.open_items.length > 0}
            <div class="open-items">
              {#each p.open_items as o}
                <div class="open-item">
                  <span>⭐ {o.item}</span>
                  {#if o.origin_date}
                    {@const days = daysAgo(o.origin_date)}
                    {#if days != null}
                      <span class="open-item-age">{days} 天前提起</span>
                    {/if}
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/each}
      {#if threads.archived.length > 0}
        <details class="archived" bind:open={showArchived}>
          <summary>已归档项目（{threads.archived.length}）</summary>
          {#each threads.archived as p (p.name)}
            <div class="project-card">
              <div class="project-name">{p.name}</div>
              {#if p.thread.length > 0}
                <div class="thread">
                  {#each sortedThread(p) as t}
                    <div class="thread-item">
                      <span class="badge">{t.date}</span>
                      <span class="thread-note">{t.note}</span>
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {/each}
        </details>
      {/if}
    {/if}
  {/if}
</div>

<style>
  .insights-head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 10px 0;
    flex-shrink: 0;
  }

  .run-group {
    margin-bottom: 14px;
  }

  .run-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 2px;
    border-bottom: 1px solid var(--border);
  }

  .run-time {
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .run-error {
    color: var(--err);
    font-size: 12px;
  }

  .empty-run {
    color: var(--text-dim);
    font-size: 13px;
    padding: 12px 4px;
  }

  .finding {
    padding: 9px 10px;
    margin: 6px 0;
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }

  .finding.dismissed {
    opacity: 0.55;
  }

  .finding-title {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }

  .finding-note {
    color: var(--text-dim);
    font-size: 13px;
    margin-top: 5px;
    line-height: 1.55;
    user-select: text;
    white-space: pre-wrap;
  }

  .finding-quote {
    margin: 8px 0 2px;
    padding: 2px 0 2px 12px;
    border-left: 3px solid var(--accent);
    color: var(--text-dim);
    font-size: 13px;
    line-height: 1.6;
    user-select: text;
    white-space: pre-wrap;
  }

  .finding-actions {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 8px;
    flex-wrap: wrap;
  }

  .finding-actions button.sm {
    padding: 3px 8px;
    font-size: 12px;
    font-weight: 400;
  }

  .project-card {
    padding: 10px 12px;
    margin-bottom: 8px;
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }

  .project-name {
    font-weight: 600;
    margin-bottom: 6px;
  }

  .thread-item {
    display: flex;
    gap: 8px;
    align-items: baseline;
    padding: 3px 0;
    font-size: 13px;
  }

  .thread-note {
    color: var(--text-dim);
    line-height: 1.55;
    user-select: text;
  }

  .open-items {
    margin-top: 8px;
    padding-top: 6px;
    border-top: 1px dashed var(--border);
  }

  .open-item {
    display: flex;
    gap: 8px;
    align-items: baseline;
    padding: 2px 0;
    font-size: 13px;
    color: var(--warn);
  }

  .open-item-age {
    color: var(--text-dim);
    font-size: 11px;
    flex-shrink: 0;
  }

  .archived {
    margin-top: 8px;
  }

  .archived summary {
    cursor: pointer;
    color: var(--text-dim);
    font-size: 13px;
    padding: 4px 2px;
  }
</style>
