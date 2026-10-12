<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { api } from "../lib/api";
  import { showToast } from "../lib/stores";
  import type { TtsChunk } from "../lib/types";

  export let entryId: number;
  // 朗读文本来源："clean" 会议纪要 / "raw" 原始内容 / "deep" 深度解析
  export let tab: string;

  const VOICES: { id: string; label: string }[] = [
    { id: "zf_001", label: "女声 1" },
    { id: "zf_002", label: "女声 2" },
    { id: "zf_003", label: "女声 3" },
    { id: "zf_004", label: "女声 4" },
    { id: "zm_009", label: "男声 1" },
    { id: "zm_010", label: "男声 2" },
    { id: "zm_011", label: "男声 3" },
    { id: "zm_012", label: "男声 4" },
  ];
  const RATES = [0.75, 1, 1.25, 1.5, 2];

  let modelReady = false;
  let checked = false;
  let voice = localStorage.getItem("masaifu_tts_voice") ?? "zf_001";
  let rate = 1;

  // idle: 未在朗读；starting: 已发起、等首句；playing: 播放中（含等后续句合成的间隙）
  let phase: "idle" | "starting" | "playing" = "idle";
  let paused = false;
  let sentences: string[] = []; // 完整分句计划（tts-plan 事件一次下发）
  let chunks: (TtsChunk | null)[] = []; // 已合成就绪的句子（tts-chunk 事件逐句到达）
  let total = 0;
  let current = 0; // 正在播/下一句要播的序号
  let backendDone = false;
  let statusText = ""; // sidecar 进度（加载模型/正在合成第 n 句…）
  let listEl: HTMLDivElement | null = null;

  let audio: HTMLAudioElement | null = null;
  let loading = false; // playIndex 进行中，防并发双播
  let objectUrl: string | null = null;
  let unlistens: UnlistenFn[] = [];

  function resetPlayback() {
    audio?.pause();
    audio = null;
    loading = false;
    if (objectUrl) {
      URL.revokeObjectURL(objectUrl);
      objectUrl = null;
    }
    sentences = [];
    chunks = [];
    total = 0;
    current = 0;
    backendDone = false;
    paused = false;
    statusText = "";
  }

  async function playIndex(i: number) {
    const chunk = chunks[i];
    if (!chunk || loading || audio) return;
    loading = true;
    try {
      const bytes = await api.ttsReadWav(chunk.wav);
      const blob = new Blob([new Uint8Array(bytes)], { type: "audio/wav" });
      objectUrl = URL.createObjectURL(blob);
      audio = new Audio(objectUrl);
      audio.playbackRate = rate;
      audio.onended = () => {
        if (objectUrl) {
          URL.revokeObjectURL(objectUrl);
          objectUrl = null;
        }
        audio = null;
        current = i + 1;
        void advance();
      };
      await audio.play();
    } catch (e) {
      console.error("[tts] 播放失败 index=", i, e);
      showToast("播放失败：" + String(e));
      await stop();
    } finally {
      loading = false;
    }
  }

  // 从 current 开始播下一句；没合成好就等 tts-chunk / tts-done 事件再推进
  async function advance() {
    if (phase === "idle") return;
    if (current < total && chunks[current]) {
      await playIndex(current);
      return; // onended 里继续推进
    }
    if (current >= total && backendDone) {
      phase = "idle";
      resetPlayback();
      return;
    }
    // 跳过合成失败的句号
    if (current < total && backendDone && !chunks[current]) {
      current += 1;
      return advance();
    }
    // 否则等待后续 chunk 事件
  }

  // 点击列表跳播某句（仅已合成就绪的可点）
  function jump(i: number) {
    if (phase === "idle" || !chunks[i] || loading) return;
    audio?.pause();
    audio = null;
    if (objectUrl) {
      URL.revokeObjectURL(objectUrl);
      objectUrl = null;
    }
    current = i;
    paused = false;
    phase = "playing";
    void advance();
  }

  async function start() {
    resetPlayback();
    phase = "starting";
    statusText = "正在启动语音合成…";
    // 结果通过 tts-plan / tts-chunk / tts-done 事件流式回来，invoke 本身等全部合成完才返回
    void api.meetingSpeak(entryId, voice, tab).catch((e) => {
      if (String(e) !== "已停止") showToast("朗读失败：" + String(e));
    });
  }

  async function stop() {
    phase = "idle";
    resetPlayback();
    await api.meetingStopSpeak().catch(() => {});
  }

  function togglePause() {
    if (!audio) return;
    if (paused) {
      void audio.play();
      paused = false;
    } else {
      audio.pause();
      paused = true;
    }
  }

  function setRate(r: number) {
    rate = r;
    if (audio) audio.playbackRate = r;
  }

  function onPlan(payload: { entryId: number; sentences: string[] }) {
    if (payload.entryId !== entryId || phase === "idle") return;
    sentences = payload.sentences;
    total = payload.sentences.length;
    chunks = new Array(total).fill(null);
  }

  function onChunk(c: TtsChunk) {
    if (c.entryId !== entryId || phase === "idle") return;
    if (chunks.length === 0) {
      // plan 事件未到达的兜底（不应发生）
      total = c.total;
      chunks = new Array(c.total).fill(null);
    }
    chunks[c.index] = c;
    chunks = chunks; // 下标赋值不触发 Svelte 响应式，手动刷新引用
    // 首句到达立即开播；等待后续句时该句到达则接续
    if (phase === "starting" || (phase === "playing" && !audio && c.index >= current)) {
      phase = "playing";
      if (!audio) void advance();
    }
  }

  onMount(async () => {
    try {
      const status = await api.ttsModelStatus();
      modelReady = status.downloaded && status.sidecarReady;
    } catch {
      modelReady = false;
    }
    checked = true;
    unlistens = await Promise.all([
      listen<{ entryId: number; sentences: string[] }>("tts-plan", (e) => onPlan(e.payload)),
      listen<TtsChunk>("tts-chunk", (e) => onChunk(e.payload)),
      listen<string>("tts-progress", (e) => {
        statusText = e.payload;
      }),
      listen<number>("tts-done", (e) => {
        if (e.payload !== entryId) return;
        backendDone = true;
        if (!audio && phase !== "idle") void advance();
      }),
      listen<number>("tts-stopped", (e) => {
        if (e.payload !== entryId) return;
        phase = "idle";
        resetPlayback();
      }),
      listen<string>("tts-error", (e) => {
        showToast("朗读失败：" + e.payload);
        phase = "idle";
        resetPlayback();
      }),
    ]);
  });

  onDestroy(() => {
    unlistens.forEach((f) => f());
    if (phase !== "idle") void api.meetingStopSpeak().catch(() => {});
    resetPlayback();
  });

  // 切换页签（纪要/原文/解析）时停止当前朗读
  let lastTab = tab;
  $: if (tab !== lastTab) {
    lastTab = tab;
    if (phase !== "idle") void stop();
  }

  // 当前句变化时滚动到可视区域
  $: if (listEl && current >= 0) {
    void tick().then(() => {
      listEl?.querySelector(".row.playing")?.scrollIntoView({ block: "nearest" });
    });
  }

  $: progressText =
    phase === "starting"
      ? statusText || "正在启动语音合成…"
      : total > 0
        ? audio
          ? `第 ${current + 1} / ${total} 句`
          : statusText || `等待合成… ${chunks.filter(Boolean).length} / ${total} 句就绪`
        : statusText;
</script>

{#if checked}
  {#if !modelReady}
    <div class="speak-bar">
      <span class="hint">朗读功能需要先到「设置 → 会议录音」下载语音合成模型</span>
    </div>
  {:else}
    <div class="speak-bar">
      {#if phase === "idle"}
        <button type="button" on:click={start}>
          ▶ 朗读{tab === "raw" ? "原文" : tab === "deep" ? "解析" : "纪要"}
        </button>
      {:else}
        {#if paused}
          <button type="button" on:click={togglePause}>▶ 继续</button>
        {:else}
          <button type="button" on:click={togglePause} disabled={!audio}>⏸ 暂停</button>
        {/if}
        <button type="button" on:click={stop}>⏹ 停止</button>
        <span class="hint">{progressText}</span>
        {#if phase === "playing" && !audio && !backendDone}
          <span class="spinner"></span>
        {/if}
      {/if}
      {#if phase === "idle"}
        <select
          bind:value={voice}
          on:change={() => localStorage.setItem("masaifu_tts_voice", voice)}
        >
          {#each VOICES as v}
            <option value={v.id}>{v.label}</option>
          {/each}
        </select>
      {:else}
        <select value={rate} on:change={(e) => setRate(Number(e.currentTarget.value))}>
          {#each RATES as r}
            <option value={r}>{r}x</option>
          {/each}
        </select>
      {/if}
    </div>
    {#if phase !== "idle" && sentences.length > 0}
      <div class="playlist" bind:this={listEl}>
        {#each sentences as s, i}
          <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
          <div
            class="row"
            class:playing={i === current && phase === "playing"}
            class:ready={!!chunks[i]}
            on:click={() => jump(i)}
          >
            <span class="row-icon">
              {#if i === current && phase === "playing" && audio}
                ▶
              {:else if chunks[i]}
                ✓
              {:else}
                …
              {/if}
            </span>
            <span class="row-text">{s}</span>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
{/if}

<style>
  .speak-bar {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 6px 0;
  }
  .speak-bar select {
    width: auto;
  }
  .playlist {
    max-height: 180px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
    margin-bottom: 6px;
  }
  .row {
    display: flex;
    gap: 8px;
    padding: 4px 10px;
    font-size: 12px;
    color: var(--text-dim);
    cursor: default;
    align-items: baseline;
  }
  .row.ready {
    color: var(--text);
    cursor: pointer;
  }
  .row.ready:hover {
    background: var(--hover, rgba(128, 128, 128, 0.12));
  }
  .row.playing {
    color: var(--accent, #4f7cff);
    font-weight: 600;
  }
  .row-icon {
    flex-shrink: 0;
    width: 14px;
  }
  .row-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
