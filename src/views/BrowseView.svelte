<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { get } from "svelte/store";
  import { api, clipText, lastWeek, reminderRuleText, reminderStatus, sourceLabel, summaryTitle, thisWeek, today } from "../lib/api";
  import EntryDetail from "../components/EntryDetail.svelte";
  import Pagination from "../components/Pagination.svelte";
  import ReminderDetail from "../components/ReminderDetail.svelte";
  import ReminderForm from "../components/ReminderForm.svelte";
  import SourceIcon from "../components/SourceIcon.svelte";
  import { renderMarkdown } from "../lib/markdown";
  import { pendingReminderFocus, showToast, view } from "../lib/stores";
  import {
    CATEGORIES,
    type EntryRow,
    type PnoteStatus,
    type ReminderRow,
    type SearchHit,
    type SummaryRow,
    type TreeStats,
  } from "../lib/types";

  let stats: TreeStats | null = null;
  let selectedKey = "all";
  let entries: EntryRow[] = [];
  let summaries: SummaryRow[] = [];
  let current: EntryRow | null = null;
  let currentSummary: SummaryRow | null = null;
  let loading = false;
  let rerunning = false;
  let searchQuery = "";
  let searchDebounce: ReturnType<typeof setTimeout> | null = null;
  let answering = false;
  let aiSearching = false;
  let aiAnswer = "";
  let reminders: ReminderRow[] = [];
  let currentReminder: ReminderRow | null = null;
  let newInspiration = "";
  let generatingDate = "";
  let nowText = "";
  let todayCount = 0;
  let importingMeeting = false;
  let meetingStage = "";
  let meetingProgress: number | null = null;
  let summaryDate = today();
  let weekChoice: "last" | "this" = "last";
  let pnoteStatus: PnoteStatus | null = null;
  let pnoteSyncing = false;
  let pnoteConnecting = false;
  let pnoteStage = "";
  let analyzingEvents = false;
  $: weekKey = weekChoice === "last" ? lastWeek() : thisWeek();

  const MEETING_STAGE_LABEL: Record<string, string> = {
    decoding: "解码音频中…",
    transcribing: "语音识别中…",
    minutes: "生成会议纪要中…",
  };

  async function importMeeting() {
    const path = await open({
      multiple: false,
      filters: [{ name: "音频文件", extensions: ["mp3", "m4a", "wav", "ogg", "opus", "flac", "aac"] }],
    });
    if (!path) return;
    importingMeeting = true;
    meetingStage = "";
    meetingProgress = null;
    try {
      await api.importMeetingAudio(path);
      showToast("会议导入完成");
      refreshStats();
      refreshCurrent();
    } catch (e) {
      showToast(String(e));
    } finally {
      importingMeeting = false;
      meetingStage = "";
      meetingProgress = null;
    }
  }

  function tickClock() {
    const d = new Date();
    const p = (n: number) => String(n).padStart(2, "0");
    nowText = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  }

  async function refreshTodayCount() {
    try {
      todayCount = await api.countTodayEntries();
    } catch {
      // 忽略统计失败
    }
  }

  const PAGE_SIZE = 20;
  let page = 1;
  let entriesTotal = 0;
  let fetchedPage = 0;

  $: recurringReminders = reminders.filter((r) => r.kind !== "once");
  $: onceReminders = reminders.filter((r) => r.kind === "once");
  $: dailySummaries = summaries.filter((s) => !/-W\d{2}$/.test(s.date));
  $: weeklySummaries = summaries.filter((s) => /-W\d{2}$/.test(s.date));
  $: pagedDaily = dailySummaries.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  $: pagedWeekly = weeklySummaries.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  $: pagedRecurring = recurringReminders.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  $: pagedOnce = onceReminders.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  $: activeSummaryNode =
    selectedKey === "sumdaily"
      ? "daily"
      : selectedKey === "sumweekly"
        ? "weekly"
        : selectedKey.startsWith("sum:")
          ? /-W\d{2}$/.test(selectedKey.slice(4))
            ? "weekly"
            : "daily"
          : "";
  $: activeReminderNode =
    selectedKey === "rem:recurring"
      ? "recurring"
      : selectedKey === "rem:once"
        ? "once"
        : currentReminder
          ? currentReminder.kind === "once"
            ? "once"
            : "recurring"
          : "";
  $: generatingKind = generatingDate
    ? /-W\d{2}$/.test(generatingDate)
      ? "weekly"
      : "daily"
    : "";

  async function addInspiration() {
    const text = newInspiration.trim();
    if (!text) return;
    try {
      const row = await api.addEntry(text);
      // 手动添加的灵感直接归入「灵感」分类，立即出现在当前列表
      await api.updateEntry(row.id, "", "灵感", "");
      newInspiration = "";
      showToast("已添加灵感");
      refreshStats();
      refreshCurrent();
    } catch (e) {
      showToast(String(e));
    }
  }

  $: recurringReminders = reminders.filter((r) => r.kind !== "once");
  $: onceReminders = reminders.filter((r) => r.kind === "once");
  $: dailySummaries = summaries.filter((s) => !/-W\d{2}$/.test(s.date));
  $: weeklySummaries = summaries.filter((s) => /-W\d{2}$/.test(s.date));

  const CATEGORY_ICON: Record<string, string> = {
    灵感: "💡",
    代码: "💻",
    文章: "📄",
    链接: "🔗",
    待办: "✅",
    会议: "🤝",
    其他: "📦",
  };
  const SOURCE_ICON: Record<string, string> = { clipboard: "📋", manual: "✏️", meeting: "🎙️" };

  function fmtKb(kb: number): string {
    if (kb >= 1048576) return (kb / 1048576).toFixed(1) + " GiB";
    if (kb >= 1024) return (kb / 1024).toFixed(1) + " MiB";
    return Math.round(kb) + " KB";
  }

  async function refreshPnoteStatus() {
    try {
      pnoteStatus = await api.pnoteStatus();
    } catch {
      // 未启用或 sidecar 未就绪时不报错，面板保持未连接态
    }
  }

  async function pnoteSyncNow() {
    if (pnoteSyncing) return;
    pnoteSyncing = true;
    pnoteStage = "正在同步…";
    try {
      await api.pnoteSyncNow();
    } catch (e) {
      showToast("同步失败：" + String(e));
    } finally {
      pnoteSyncing = false;
      pnoteStage = "";
      refreshPnoteStatus();
    }
  }

  // 断开/休眠状态下手动触发一次扫描连接，不等守护循环的退避间隔
  async function pnoteConnectNow() {
    if (pnoteConnecting) return;
    pnoteConnecting = true;
    try {
      const s = await api.pnoteConnectNow();
      pnoteStatus = s;
      showToast(s.connected ? "已连接录音卡" : "未找到设备，请按录音卡按键唤醒后重试");
    } catch (e) {
      showToast("连接失败：" + String(e));
    } finally {
      pnoteConnecting = false;
    }
  }

  async function analyzeEvents() {
    if (analyzingEvents) return;
    analyzingEvents = true;
    try {
      const n = await api.analyzeRecorderEvents();
      showToast(n > 0 ? `已创建 ${n} 条提醒` : "没有识别到值得提醒的事件");
      refreshStats();
      loadReminders();
    } catch (e) {
      showToast(String(e));
    } finally {
      analyzingEvents = false;
    }
  }

  // 选中录音卡来源时刷新一次设备状态
  $: if (selectedKey === "source:recorder") refreshPnoteStatus();

  onMount(async () => {
    await refreshStats();
    try {
      summaries = await api.listSummaries();
    } catch (e) {
      showToast("读取汇总列表失败：" + String(e));
    }
    loadReminders();
    const unlisten = listen("reminder-fired", () => loadReminders());
    const unlistenSummary = listen<SummaryRow>("summary-done", async (e) => {
      generatingDate = "";
      try {
        summaries = await api.listSummaries();
      } catch {
        // 忽略刷新失败
      }
      const row = e.payload;
      if (row && currentSummary?.date === row.date) {
        currentSummary = row;
      }
    });
    const unlistenStarted = listen<string>("summary-started", (e) => {
      generatingDate = e.payload;
    });
    const unlistenMeeting = listen<{ stage: string; message: string; progress: number | null }>("meeting-progress", (e) => {
      if (e.payload.stage === "done" || e.payload.stage === "error") {
        meetingStage = "";
        meetingProgress = null;
      } else {
        meetingStage = e.payload.message || MEETING_STAGE_LABEL[e.payload.stage] || "";
        meetingProgress = e.payload.progress ?? null;
      }
    });
    const unlistenFailed = listen<string>("summary-failed", (e) => {
      if (generatingDate === e.payload) generatingDate = "";
    });
    const unlisten2 = listen("show-reminders", () => {
      pendingReminderFocus.set(false);
      focusReminders();
    });
    const unlistenPnoteState = listen<PnoteStatus>("pnote-state", (e) => {
      pnoteStatus = e.payload;
    });
    const unlistenPnoteSynced = listen<{ newEntries?: number; name?: string }>("pnote-synced", (e) => {
      const n = e.payload?.newEntries;
      showToast(n != null ? `新增 ${n} 条录音，已转写入库` : `录音 ${e.payload?.name ?? ""} 已同步，待转写`);
      refreshCurrent();
      refreshStats();
      refreshPnoteStatus();
    });
    const unlistenPnoteProgress = listen<{ file?: string; received?: number; total?: number }>("pnote-progress", (e) => {
      const { file, received, total } = e.payload ?? {};
      pnoteStage =
        total && received != null
          ? `正在下载 ${file ?? ""} ${Math.min(100, Math.round((received / total) * 100))}%`
          : `正在下载 ${file ?? ""}…`;
    });
    if (get(pendingReminderFocus)) {
      pendingReminderFocus.set(false);
      focusReminders();
    }
    load("all");
    tickClock();
    refreshTodayCount();
    const clockTimer = setInterval(tickClock, 1000);
    const countTimer = setInterval(refreshTodayCount, 30000);
    return () => {
      clearInterval(clockTimer);
      clearInterval(countTimer);
      unlisten.then((f) => f());
      unlistenSummary.then((f) => f());
      unlistenStarted.then((f) => f());
      unlistenMeeting.then((f) => f());
      unlistenFailed.then((f) => f());
      unlisten2.then((f) => f());
      unlistenPnoteState.then((f) => f());
      unlistenPnoteSynced.then((f) => f());
      unlistenPnoteProgress.then((f) => f());
    };
  });

  async function loadReminders() {
    try {
      reminders = await api.listReminders();
      if (currentReminder) {
        currentReminder =
          reminders.find((r) => r.id === currentReminder!.id) ?? null;
        if (!currentReminder && selectedKey.startsWith("rem:")) {
          selectedKey = "rem:new";
        }
      }
    } catch (e) {
      showToast("读取提醒列表失败：" + String(e));
    }
  }

  function focusReminders() {
    selectNewReminder();
  }

  function selectNewReminder() {
    searchQuery = "";
    aiAnswer = "";
    current = null;
    currentSummary = null;
    currentReminder = null;
    selectedKey = "rem:new";
  }

  function selectReminder(r: ReminderRow) {
    searchQuery = "";
    aiAnswer = "";
    current = null;
    currentSummary = null;
    currentReminder = r;
    selectedKey = "rem:" + r.id;
  }

  function selectSummaryList(kind: "daily" | "weekly") {
    searchQuery = "";
    aiAnswer = "";
    current = null;
    currentSummary = null;
    currentReminder = null;
    page = 1;
    selectedKey = kind === "daily" ? "sumdaily" : "sumweekly";
  }

  function selectReminderList(kind: "recurring" | "once") {
    searchQuery = "";
    aiAnswer = "";
    current = null;
    currentSummary = null;
    currentReminder = null;
    page = 1;
    selectedKey = "rem:" + kind;
  }

  async function refreshStats() {
    try {
      stats = await api.getTreeStats();
    } catch (e) {
      showToast("读取统计失败：" + String(e));
    }
    refreshTodayCount();
  }

  async function load(key: string, value?: string, query?: string) {
    selectedKey = key + (value ? ":" + value : "");
    current = null;
    currentSummary = null;
    currentReminder = null;
    aiAnswer = "";
    page = 1;
    await fetchEntries(key, value, query, 1);
  }

  async function fetchEntries(key: string, value: string | undefined, query: string | undefined, p: number) {
    fetchedPage = p;
    loading = true;
    try {
      const cat = key === "category" ? value : undefined;
      const src = key === "source" ? value : undefined;
      const res = await api.listEntriesFiltered(cat, src, query || undefined, PAGE_SIZE, (p - 1) * PAGE_SIZE);
      entries = res.entries;
      entriesTotal = res.total;
    } catch (e) {
      showToast("读取条目失败：" + String(e));
      entries = [];
      entriesTotal = 0;
    } finally {
      loading = false;
    }
    // 直接搜索有结果则不启动 AI 流程；无结果时由 AI 提取搜索词重新搜索
    if (p === 1 && query && query.trim() && entries.length === 0) {
      await aiFallbackSearch(query.trim());
    }
  }

  // AI 兜底搜索：提取搜索词 → 每个词单独全局搜索取前 5 条 → 排重 → 基于结果生成 AI 回答
  async function aiFallbackSearch(q: string) {
    aiSearching = true;
    try {
      const terms = await api.aiSearchTerms(q);
      if (searchQuery.trim() !== q) return;
      const seen = new Set<number>();
      const merged: EntryRow[] = [];
      for (const t of terms) {
        const res = await api.listEntriesFiltered(undefined, undefined, t, 5, 0);
        for (const e of res.entries) {
          if (!seen.has(e.id)) {
            seen.add(e.id);
            merged.push(e);
          }
        }
      }
      if (searchQuery.trim() !== q) return;
      entries = merged;
      entriesTotal = merged.length;
      if (merged.length > 0) {
        answering = true;
        try {
          aiAnswer = await api.aiAnswer(q, merged.slice(0, 10).map(entryToHit));
        } catch (e) {
          showToast("AI 回答失败：" + String(e));
        } finally {
          answering = false;
        }
      }
    } catch (e) {
      showToast("AI 搜索失败：" + String(e));
    } finally {
      aiSearching = false;
    }
  }

  // 条目类列表（全部 / 分类 / 来源）翻页时按页重新查询
  $: isEntryList =
    !current &&
    !currentSummary &&
    !currentReminder &&
    (selectedKey === "all" ||
      selectedKey.startsWith("category:") ||
      selectedKey.startsWith("source:"));
  $: if (isEntryList && page !== fetchedPage) {
    const parts = selectedKey.split(":");
    fetchEntries(parts[0], parts.slice(1).join(":") || undefined, searchQuery, page);
  }

  async function selectSummaryDate(date: string) {
    selectedKey = "sum:" + date;
    current = null;
    currentSummary = null;
    currentReminder = null;
    try {
      currentSummary = await api.getSummary(date);
    } catch (e) {
      showToast("读取汇总失败：" + String(e));
    }
  }

  function refreshCurrent() {
    const parts = selectedKey.split(":");
    const key = parts[0];
    const value = parts.slice(1).join(":");
    if (key === "sum") {
      selectSummaryDate(value);
    } else {
      load(key, key === "all" ? undefined : value, searchQuery);
    }
  }

  function onSearchInput() {
    aiAnswer = "";
    if (searchDebounce) clearTimeout(searchDebounce);
    searchDebounce = setTimeout(() => {
      const parts = selectedKey.split(":");
      const key = parts[0];
      const value = parts.slice(1).join(":");
      if (key === "sum") return;
      load(key, key === "all" ? undefined : value, searchQuery);
    }, 250);
  }

  function clearSearch() {
    searchQuery = "";
    aiAnswer = "";
    const parts = selectedKey.split(":");
    const key = parts[0];
    const value = parts.slice(1).join(":");
    if (key !== "sum") load(key, key === "all" ? undefined : value, "");
  }

  function selectNode(key: string, value?: string) {
    searchQuery = "";
    load(key, value, "");
  }

  function entryToHit(e: EntryRow): SearchHit {
    return {
      docType: "entry",
      docId: String(e.id),
      title: e.title || e.rawContent.slice(0, 40),
      content: e.cleanContent || e.rawContent,
      category: e.category || "",
      tags: e.tags || "",
      source: e.source,
      createdAt: e.createdAt,
      rank: 0,
    };
  }

  async function askAi() {
    if (!searchQuery.trim() || entries.length === 0) return;
    answering = true;
    aiAnswer = "";
    try {
      aiAnswer = await api.aiAnswer(searchQuery, entries.slice(0, 10).map(entryToHit));
    } catch (e) {
      aiAnswer = "";
      showToast("AI 回答失败：" + String(e));
    } finally {
      answering = false;
    }
  }

  async function rerun(date: string) {
    rerunning = true;
    showToast(`正在重新生成${summaryTitle(date)}，可能需要几十秒…`);
    try {
      const row = await api.rerunSummary(date);
      summaries = await api.listSummaries();
      if (selectedKey === "sum:" + date) {
        currentSummary = row;
      }
      showToast(`已重新生成${summaryTitle(date)}`);
      refreshStats();
    } catch (e) {
      showToast("重跑失败：" + String(e));
    } finally {
      rerunning = false;
    }
  }
</script>

<div class="full-layout">
  <div class="side tree-side">
    <div class="tree-scroll">
    <div class="tree-item" class:active={selectedKey === "all"} on:click={() => selectNode("all")}>
      <span class="tree-label"><span class="tree-icon">▤</span><span class="txt">全部条目</span></span>
      <span class="tree-count">{stats?.totalEntries ?? 0}</span>
    </div>

    <div class="tree-head"><span>分类</span></div>
    {#each CATEGORIES as c}
      {@const count = stats?.byCategory.find((x) => x.key === c)?.count ?? 0}
      <div
        class="tree-item indent"
        class:active={selectedKey === `category:${c}`}
        on:click={() => selectNode("category", c)}
      >
        <span class="tree-label"><span class="tree-icon">{CATEGORY_ICON[c] ?? "📦"}</span><span class="txt">{c}</span></span>
        <span class="tree-count">{count}</span>
      </div>
    {/each}

    <div class="tree-head"><span>来源</span></div>
    {#each ["clipboard", "manual", "meeting", "recorder"] as s}
      {@const count = stats?.bySource.find((x) => x.key === s)?.count ?? 0}
      <div
        class="tree-item indent"
        class:active={selectedKey === `source:${s}`}
        on:click={() => selectNode("source", s)}
      >
        <span class="tree-label"><span class="tree-icon"><SourceIcon source={s} emoji={SOURCE_ICON[s] ?? "🗂"} /></span><span class="txt">{sourceLabel(s)}</span></span>
        <span class="tree-count">
          {count}
          {#if s === "recorder" && pnoteStatus && pnoteStatus.pending > 0}
            <span class="pnote-pending" title="录音卡中待同步的录音">+{pnoteStatus.pending}</span>
          {/if}
        </span>
      </div>
    {/each}

    <div class="tree-head"><span>汇总</span></div>
    <div
      class="tree-item indent"
      class:active={activeSummaryNode === "daily"}
      on:click={() => selectSummaryList("daily")}
    >
      <span class="tree-label"><span class="tree-icon">📅</span><span class="txt">每日汇总</span></span>
      <span style="display:flex;align-items:center;gap:2px;">
        <button
          class="icon-btn"
          title={generatingKind === "daily" ? "正在生成汇总…" : "重新生成今天汇总"}
          on:click|stopPropagation={() => rerun(today())}
          disabled={rerunning || generatingKind === "daily"}
        >
          {#if rerunning || generatingKind === "daily"}
            <span class="spinner" style="margin-right:0;"></span>
          {:else}
            ↻
          {/if}
        </button>
        <span class="tree-count">{dailySummaries.length}</span>
      </span>
    </div>
    <div
      class="tree-item indent"
      class:active={activeSummaryNode === "weekly"}
      on:click={() => selectSummaryList("weekly")}
    >
      <span class="tree-label"><span class="tree-icon">📆</span><span class="txt">每周汇总</span></span>
      <span style="display:flex;align-items:center;gap:6px;">
        {#if generatingKind === "weekly"}
          <span class="spinner" style="margin-right:0;" title="正在生成周报…"></span>
        {/if}
        <span class="tree-count">{weeklySummaries.length}</span>
      </span>
    </div>

    <div class="tree-head"><span>定时任务</span></div>
    <div
      class="tree-item indent"
      class:active={selectedKey === "rem:new"}
      on:click={selectNewReminder}
    >
      <span class="tree-label"><span class="tree-icon">＋</span><span class="txt">新建任务</span></span>
    </div>
    <div
      class="tree-item indent"
      class:active={activeReminderNode === "recurring"}
      on:click={() => selectReminderList("recurring")}
    >
      <span class="tree-label"><span class="tree-icon">🔁</span><span class="txt">循环任务</span></span>
      <span class="tree-count">{recurringReminders.length}</span>
    </div>
    <div
      class="tree-item indent"
      class:active={activeReminderNode === "once"}
      on:click={() => selectReminderList("once")}
    >
      <span class="tree-label"><span class="tree-icon">⏰</span><span class="txt">一次性任务</span></span>
      <span class="tree-count">{onceReminders.length}</span>
    </div>
    </div>

    <div class="sidebar-footer">
      <div class="status-line">
        <span>{nowText}</span>
        <span>今日收集 {todayCount} 条</span>
      </div>
      <div style="display:flex;gap:6px;">
        <button class="sm" style="flex:1;" on:click={() => view.set("settings")}>⚙ 设置</button>
        <button class="sm" style="flex:1;" on:click={() => view.set("about")}>关于</button>
      </div>
    </div>
  </div>

  <div class="main-area">
    {#if currentReminder}
      {#key currentReminder.id}
        <ReminderDetail
          reminder={currentReminder}
          onChanged={loadReminders}
          onDeleted={() => {
            const kind = currentReminder?.kind === "once" ? "once" : "recurring";
            selectReminderList(kind);
            loadReminders();
          }}
          onBack={() => {
            const kind = currentReminder?.kind === "once" ? "once" : "recurring";
            selectReminderList(kind);
          }}
        />
      {/key}
    {:else if selectedKey === "rem:new"}
      <ReminderForm
        onSaved={(r) => {
          loadReminders();
          if (r) selectReminder(r);
        }}
      />
    {:else if selectedKey === "rem:recurring" || selectedKey === "rem:once"}
      <div class="results" style="padding:10px;">
        {#each selectedKey === "rem:once" ? pagedOnce : pagedRecurring as r (r.id)}
          <button type="button" class="result-item" on:click={() => selectReminder(r)}>
            <div class="result-title">
              <span class="dot" class:off={!r.enabled}></span>
              <span>{r.title}</span>
              <span class="badge">{reminderStatus(r)}</span>
            </div>
            <div class="result-meta">
              <span>{reminderRuleText(r)}</span>
              {#if r.enabled}
                <span>下次：{r.nextFireAt.slice(0, 16)}</span>
              {/if}
            </div>
            {#if r.note}
              <div class="result-preview">{clipText(r.note, 120)}</div>
            {/if}
          </button>
        {:else}
          <div class="empty">
            暂无{selectedKey === "rem:once" ? "一次性" : "循环"}任务，点左侧「新建任务」创建
          </div>
        {/each}
      </div>
      <Pagination
        total={selectedKey === "rem:once" ? onceReminders.length : recurringReminders.length}
        bind:page
        pageSize={PAGE_SIZE}
      />
    {:else if selectedKey === "sumdaily" || selectedKey === "sumweekly"}
      {#if selectedKey === "sumdaily"}
        <div style="display:flex;align-items:center;gap:10px;padding:10px 10px 0;flex-shrink:0;">
          <input type="date" bind:value={summaryDate} style="width:150px;" />
          <button
            class="primary"
            on:click={() => rerun(summaryDate)}
            disabled={!summaryDate || rerunning || generatingDate === summaryDate}
          >
            {rerunning || generatingDate === summaryDate ? "生成中…" : "立即汇总"}
          </button>
          <span class="hint">选择日期，基于当天已收集的条目生成每日汇总</span>
        </div>
      {:else}
        <div style="display:flex;align-items:center;gap:10px;padding:10px 10px 0;flex-shrink:0;">
          <select bind:value={weekChoice} style="width:180px;">
            <option value="last">上周（{lastWeek()}）</option>
            <option value="this">本周（{thisWeek()}）</option>
          </select>
          <button
            class="primary"
            on:click={() => rerun(weekKey)}
            disabled={rerunning || generatingDate === weekKey}
          >
            {rerunning || generatingDate === weekKey ? "生成中…" : "立即汇总"}
          </button>
          <span class="hint">汇总所选周（周一至周日）的条目</span>
        </div>
      {/if}
      <div class="results" style="padding:10px;">
        {#each selectedKey === "sumdaily" ? pagedDaily : pagedWeekly as s (s.date)}
          <button type="button" class="result-item" on:click={() => selectSummaryDate(s.date)}>
            <div class="result-title">
              <span class="badge summary">{selectedKey === "sumdaily" ? "每日" : "每周"}</span>
              <span>{summaryTitle(s.date)}</span>
            </div>
            <div class="result-meta">
              <span>生成于 {s.createdAt.slice(0, 16)}</span>
            </div>
            <div class="result-preview">{clipText(s.content, 160)}</div>
          </button>
        {:else}
          <div class="empty">暂无{selectedKey === "sumdaily" ? "每日" : "每周"}汇总</div>
        {/each}
      </div>
      <Pagination
        total={selectedKey === "sumdaily" ? dailySummaries.length : weeklySummaries.length}
        bind:page
        pageSize={PAGE_SIZE}
      />
    {:else if current}
      {#key current.id}
        <EntryDetail
          entry={current}
          onSaved={() => {
            current = null;
            refreshCurrent();
          }}
          onDeleted={() => {
            current = null;
            refreshCurrent();
            refreshStats();
            showToast("已删除");
          }}
          onBack={() => {
            current = null;
          }}
        />
      {/key}
    {:else if currentSummary}
      <div
        style="display:flex;align-items:center;gap:10px;padding:10px 22px;border-bottom:1px solid var(--border);"
      >
        <strong>{summaryTitle(currentSummary.date)}</strong>
        <button on:click={() => rerun(currentSummary.date)} disabled={rerunning}>
          {rerunning ? "生成中…" : "重新生成"}
        </button>
        <span class="hint">{currentSummary.markdownPath}</span>
      </div>
      <div class="summary-content">
        {@html renderMarkdown(currentSummary.content)}
      </div>
    {:else}
      {#if selectedKey === "category:灵感" || selectedKey === "source:manual"}
        <div style="display:flex;gap:8px;padding:10px 10px 0;flex-shrink:0;">
          <input
            type="text"
            style="flex:1;"
            placeholder="记录一条灵感，回车添加…"
            bind:value={newInspiration}
            on:keydown={(e) => e.key === "Enter" && addInspiration()}
          />
          <button class="primary" on:click={addInspiration} disabled={!newInspiration.trim()}>
            添加灵感
          </button>
        </div>
      {/if}
      {#if selectedKey === "source:meeting" || selectedKey === "category:会议"}
        <div style="display:flex;gap:8px;align-items:center;padding:10px 10px 0;flex-shrink:0;">
          <button class="primary" on:click={importMeeting} disabled={importingMeeting}>
            导入音频文件
          </button>
          {#if importingMeeting}
            <span class="spinner"></span>
            <div class="meeting-progress">
              <span class="hint">{meetingStage || "准备导入…"}</span>
              <div class="progress-track">
                {#if meetingProgress != null}
                  <div class="progress-fill" style="width:{Math.round(meetingProgress * 100)}%"></div>
                {:else}
                  <div class="progress-fill indeterminate"></div>
                {/if}
              </div>
            </div>
          {/if}
        </div>
      {/if}
      {#if selectedKey === "source:recorder"}
        <div class="pnote-panel">
          {#if pnoteStatus?.connected}
            <span class="dot ok"></span>
            <span>
              已连接 · 电量 {pnoteStatus.battery ?? "—"}{pnoteStatus.charging ? "⚡" : ""}%
              {#if pnoteStatus.capacity} · 剩余容量 {fmtKb(pnoteStatus.capacity[0])}{/if}
              {#if pnoteStatus.version} · 固件 {pnoteStatus.version}{/if}
            </span>
          {:else if pnoteStatus?.sleeping}
            <span class="dot warn"></span>
            <span>设备休眠，按录音卡按键唤醒后自动连接</span>
          {:else}
            <span class="dot off"></span>
            <span>未连接（在「设置 → 录音卡」启用）</span>
          {/if}
          {#if pnoteStatus?.running && !pnoteStatus?.connected}
            <button on:click={pnoteConnectNow} disabled={pnoteConnecting}>
              {pnoteConnecting ? "连接中…" : "连接录音卡"}
            </button>
          {/if}
          <button
            class="primary"
            on:click={pnoteSyncNow}
            disabled={pnoteSyncing || !pnoteStatus?.connected}
          >
            立即同步
          </button>
          {#if pnoteSyncing}
            <span class="spinner"></span>
            <span class="hint">{pnoteStage || "同步中…"}</span>
          {/if}
          <button on:click={analyzeEvents} disabled={analyzingEvents}>
            {analyzingEvents ? "分析中…" : "分析录音事件"}
          </button>
          {#if analyzingEvents}
            <span class="spinner"></span>
          {/if}
        </div>
      {/if}
      <div style="display:flex;gap:8px;padding:10px 10px 0;flex-shrink:0;">
        <input
          type="text"
          style="flex:1;"
          placeholder="在当前分类中搜索标题 / 内容 / 标签…"
          bind:value={searchQuery}
          on:input={onSearchInput}
        />
        <button
          class="primary"
          on:click={askAi}
          disabled={answering || !searchQuery.trim() || entries.length === 0}
        >
          {answering ? "思考中…" : "AI 回答"}
        </button>
        {#if searchQuery}
          <button class="icon-btn" title="清除搜索" on:click={clearSearch}>✕</button>
        {/if}
      </div>
      {#if aiSearching}
        <div class="ai-answer">
          <span class="spinner"></span>直接搜索未命中，AI 正在提取关键词重新搜索…
        </div>
      {:else if answering}
        <div class="ai-answer">
          <span class="spinner"></span>正在基于当前节点内命中的结果生成回答…
        </div>
      {:else if aiAnswer}
        <div class="ai-answer">{aiAnswer}</div>
      {/if}
      {#if loading}
        <div class="loading"><span class="spinner"></span>加载中…</div>
      {:else if aiSearching}
        <div class="loading"><span class="spinner"></span>AI 搜索中…</div>
      {:else}
        <div class="results" style="padding:10px;">
          {#if entries.length === 0}
            <div class="empty">{searchQuery ? "没有匹配的条目" : "该分类下暂无条目"}</div>
          {:else}
            {#each entries as e (e.id)}
              <button type="button" class="result-item" on:click={() => (current = e)}>
                <div class="result-title">
                  <span class="badge {e.source}">{sourceLabel(e.source)}</span>
                  <span>{e.title || clipText(e.rawContent, 40)}</span>
                  {#if e.category}
                    <span class="badge">{e.category}</span>
                  {/if}
                </div>
                <div class="result-meta">
                  <span>{e.createdAt.slice(0, 16)}</span>
                  {#if e.tags}
                    <span>{e.tags}</span>
                  {/if}
                </div>
                <div class="result-preview">{clipText(e.cleanContent || e.rawContent, 180)}</div>
              </button>
            {/each}
          {/if}
        </div>
        <Pagination total={entriesTotal} bind:page pageSize={PAGE_SIZE} />
      {/if}
    {/if}
  </div>
</div>

<style>
  .sidebar-footer {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 6px 8px;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }

  .status-line {
    display: flex;
    justify-content: space-between;
    padding: 0 2px;
    font-size: 11px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  button.sm {
    padding: 3px 8px;
    font-size: 12px;
    font-weight: 400;
  }

  .pnote-panel {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 10px 10px 0;
    flex-shrink: 0;
    flex-wrap: wrap;
    font-size: 13px;
  }

  .pnote-panel .dot.ok {
    background: var(--ok);
  }

  .pnote-panel .dot.warn {
    background: var(--warn);
  }

  .pnote-pending {
    color: var(--warn);
    font-size: 11px;
    margin-left: 4px;
  }
</style>
