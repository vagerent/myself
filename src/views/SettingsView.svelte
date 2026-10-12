<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { api } from "../lib/api";
  import { loadSettings, showToast, settings, view } from "../lib/stores";
  import type { AsrModelStatus, PnoteStatus, Settings, TtsModelStatus } from "../lib/types";

  let form: Settings = {
    apiKey: "",
    baseUrl: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
    summaryTime: "00:15",
    idleThresholdMin: 5,
    markdownDir: "",
    promptTemplate: "",
    hotkey: "ctrl+alt+space",
    autostart: false,
    aiAnswerEnabled: true,
    pollIntervalMs: 500,
    theme: "light",
    weeklySummaryDay: 1,
    weeklySummaryTime: "00:15",
    weeklyPromptTemplate: "",
    dedupScope: "day",
    meetingPromptTemplate: "",
    meetingDeepPromptTemplate: "",
    clipboardEnabled: true,
    pnoteEnabled: false,
    pnoteKeepDays: 30,
    pnoteSyncWindowDays: 2,
    pnoteDeviceAddress: "",
    recorderEventPrompt: "",
    recorderMinutesPrompt: "",
    recorderDeepPrompt: "",
    recorderAnalysisIntervalHours: 3,
  };
  let saving = false;
  let asrStatus: AsrModelStatus | null = null;
  let downloadingModel = false;
  let downloadText = "";
  let ttsStatus: TtsModelStatus | null = null;
  let downloadingTts = false;
  let ttsDownloadText = "";

  // 设置分类页签
  const sections = [
    { id: "ai", label: "AI 服务" },
    { id: "summary", label: "汇总" },
    { id: "meeting", label: "会议录音" },
    { id: "pnote", label: "录音卡" },
    { id: "general", label: "通用" },
  ] as const;
  let section: (typeof sections)[number]["id"] = "ai";

  $: if ($settings) {
    form = $settings;
  }

  async function save() {
    saving = true;
    try {
      await api.saveSettings(form);
      await loadSettings();
      // 录音卡同步窗口等即时生效项推给 sidecar（未启用时后端静默跳过）
      api.pnoteApplySettings().catch(() => {});
      showToast("设置已保存");
    } catch (e) {
      showToast("保存失败：" + String(e));
    } finally {
      saving = false;
    }
  }

  async function refreshAsrStatus() {
    try {
      asrStatus = await api.asrModelStatus();
    } catch (e) {
      showToast("读取语音识别模型状态失败：" + String(e));
    }
  }

  async function downloadModel() {
    downloadingModel = true;
    downloadText = "";
    try {
      await api.downloadAsrModel();
      await refreshAsrStatus();
      showToast("语音识别模型下载完成");
    } catch (e) {
      showToast("下载失败：" + String(e));
    } finally {
      downloadingModel = false;
      downloadText = "";
    }
  }

  async function refreshTtsStatus() {
    try {
      ttsStatus = await api.ttsModelStatus();
    } catch (e) {
      showToast("读取语音合成模型状态失败：" + String(e));
    }
  }

  async function downloadTts() {
    downloadingTts = true;
    ttsDownloadText = "";
    try {
      await api.downloadTtsModel();
      await refreshTtsStatus();
      showToast("语音合成模型下载完成");
    } catch (e) {
      showToast("下载失败：" + String(e));
    } finally {
      downloadingTts = false;
      ttsDownloadText = "";
    }
  }

  async function togglePnote(enabled: boolean) {
    try {
      await api.pnoteSetEnabled(enabled);
      showToast(enabled ? "录音卡同步已启用" : "录音卡同步已停用");
    } catch (e) {
      showToast("录音卡设置失败：" + String(e));
    }
  }

  // 当前已连接录音卡的地址（pnote-status 快照 / pnote-state 事件），绑定按钮可用与否取决于它
  let pnoteConnectedAddr: string | null = null;
  let pnoteBinding = false;

  async function refreshPnoteConnected() {
    try {
      const s = await api.pnoteStatus();
      pnoteConnectedAddr = s.deviceAddress;
    } catch {
      pnoteConnectedAddr = null;
    }
  }

  // 浏览页连接/断开录音卡时实时刷新，不必离开再进设置页
  function onPnoteState(e: { payload: PnoteStatus }) {
    pnoteConnectedAddr = e.payload.deviceAddress;
  }

  async function bindPnote() {
    pnoteBinding = true;
    try {
      const addr = await api.pnoteBindDevice();
      await loadSettings();
      showToast("已绑定录音卡 " + addr);
    } catch (e) {
      showToast("绑定失败：" + String(e));
    } finally {
      pnoteBinding = false;
    }
  }

  async function unbindPnote() {
    pnoteBinding = true;
    try {
      await api.pnoteUnbindDevice();
      await loadSettings();
      showToast("已解绑，恢复按广播名匹配");
    } catch (e) {
      showToast("解绑失败：" + String(e));
    } finally {
      pnoteBinding = false;
    }
  }

  // 进入录音卡设置页时刷新一次连接状态（绑定按钮依赖）
  $: if (section === "pnote") refreshPnoteConnected();

  onMount(() => {
    refreshAsrStatus();
    refreshTtsStatus();
    const unlistenDownload = listen<{ file: string; downloaded: number; total: number }>(
      "asr-download-progress",
      (e) => {
        const { downloaded, total } = e.payload;
        downloadText =
          total > 0
            ? `已下载 ${Math.floor((downloaded / total) * 100)}%`
            : `已下载 ${(downloaded / 1024 / 1024).toFixed(1)} MB`;
      },
    );
    const unlistenTts = listen<{ file: string; downloaded: number; total: number }>(
      "tts-download-progress",
      (e) => {
        const { downloaded, total } = e.payload;
        ttsDownloadText =
          total > 0
            ? `已下载 ${Math.floor((downloaded / total) * 100)}%`
            : `已下载 ${(downloaded / 1024 / 1024).toFixed(1)} MB`;
      },
    );
    const unlistenPnote = listen<PnoteStatus>("pnote-state", onPnoteState);
    return () => {
      unlistenDownload.then((f) => f());
      unlistenTts.then((f) => f());
      unlistenPnote.then((f) => f());
    };
  });
</script>

<div
  style="display:flex;align-items:center;gap:10px;padding:10px 22px;border-bottom:1px solid var(--border);flex-shrink:0;"
>
  <button on:click={() => view.set("browse")}>← 返回分类浏览</button>
  <strong>设置</strong>
</div>

<div class="settings-body">
  <nav class="settings-nav">
    {#each sections as s}
      <button
        class="settings-nav-item"
        class:active={section === s.id}
        on:click={() => (section = s.id)}
      >
        {s.label}
      </button>
    {/each}
  </nav>

  <div class="settings-form">
    {#if section === "ai"}
      <div class="field">
        <label>OpenAI API Key</label>
        <input type="password" bind:value={form.apiKey} placeholder="sk-…" />
        <span class="hint">仅保存在本机应用数据目录。留空则跳过 AI 预处理、汇总与回答功能。</span>
      </div>

      <div class="field">
        <label>API Base URL</label>
        <input type="text" bind:value={form.baseUrl} />
        <span class="hint">默认 OpenAI 官方地址；使用代理或兼容服务时修改，如 https://api.openai-proxy.com/v1</span>
      </div>

      <div class="field">
        <label>模型</label>
        <input type="text" bind:value={form.model} />
      </div>

      <div class="field">
        <label>开关</label>
        <div class="field-row">
          <label style="display:flex;gap:6px;align-items:center;">
            <input type="checkbox" bind:checked={form.aiAnswerEnabled} />
            AI 回答（搜索时基于结果回答）
          </label>
        </div>
      </div>
    {:else if section === "summary"}
      <div style="display:flex;gap:20px;flex-wrap:wrap;">
        <div class="field">
          <label>每日汇总时间（HH:MM，汇总前一天）</label>
          <input type="time" bind:value={form.summaryTime} style="width:140px;" />
        </div>
        <div class="field">
          <label>每周汇总（汇总上周一至周日）</label>
          <div class="field-row">
            <select bind:value={form.weeklySummaryDay} style="width:110px;">
              <option value={1}>周一</option>
              <option value={2}>周二</option>
              <option value={3}>周三</option>
              <option value={4}>周四</option>
              <option value={5}>周五</option>
              <option value={6}>周六</option>
              <option value={7}>周日</option>
            </select>
            <input type="time" bind:value={form.weeklySummaryTime} style="width:140px;" />
          </div>
        </div>
        <div class="field">
          <label>空闲阈值（分钟，触发 AI 预处理）</label>
          <input type="number" min="1" bind:value={form.idleThresholdMin} style="width:140px;" />
        </div>
      </div>

      <div class="field">
        <label>Markdown 汇总输出目录</label>
        <input type="text" bind:value={form.markdownDir} placeholder="留空则使用 文档\Masaifu\daily" />
      </div>

      <div class="field">
        <label>每日汇总提示词模板</label>
        <textarea bind:value={form.promptTemplate}></textarea>
        <span class="hint">占位符：{"{date}"}（日期）、{"{entries}"}（条目列表）。</span>
      </div>

      <div class="field">
        <label>每周汇总提示词模板</label>
        <textarea bind:value={form.weeklyPromptTemplate}></textarea>
        <span class="hint">
          占位符：{"{week}"}（周标识，如 2026-W32）、{"{range}"}（日期范围）、{"{entries}"}（条目列表）。
        </span>
      </div>
    {:else if section === "meeting"}
      <div class="field">
        <label>会议纪要提示词模板</label>
        <textarea bind:value={form.meetingPromptTemplate}></textarea>
        <span class="hint">会议转写全文（含说话人与时间戳）会自动拼接在模板之后一起发给模型。</span>
      </div>

      <div class="field">
        <label>会议深度解析提示词模板</label>
        <textarea bind:value={form.meetingDeepPromptTemplate} rows="10"></textarea>
        <span class="hint">作为系统提示词整体发给模型，会议转写全文作为用户消息。在条目详情的「深度解析」页签中使用。</span>
      </div>

      <div class="field">
        <label>语音识别模型</label>
        <div class="field-row">
          {#if asrStatus}
            <span class="hint">{asrStatus.downloaded ? "已就绪" : "模型未下载"}</span>
          {:else}
            <span class="hint">读取状态中…</span>
          {/if}
          <button on:click={downloadModel} disabled={downloadingModel || asrStatus?.downloaded}>
            {downloadingModel ? "下载中…" : "下载模型"}
          </button>
          {#if downloadingModel && downloadText}
            <span class="hint">{downloadText}</span>
          {/if}
        </div>
        {#if asrStatus}
          <span class="hint">模型存放路径：{asrStatus.path}</span>
          {#if !asrStatus.sidecarReady}
            <span class="hint">转写组件未安装，请按 tools/build_sidecar.md 构建</span>
          {/if}
        {/if}
        <span class="hint">包含语音识别（SenseVoice）、端点检测（FSMN-VAD）与说话人分离（CAM++）模型。</span>
      </div>

      <div class="field">
        <label>语音合成模型（朗读）</label>
        <div class="field-row">
          {#if ttsStatus}
            <span class="hint">{ttsStatus.downloaded ? "已就绪" : "模型未下载"}</span>
          {:else}
            <span class="hint">读取状态中…</span>
          {/if}
          <button on:click={downloadTts} disabled={downloadingTts || ttsStatus?.downloaded}>
            {downloadingTts ? "下载中…" : "下载模型"}
          </button>
          {#if downloadingTts && ttsDownloadText}
            <span class="hint">{ttsDownloadText}</span>
          {/if}
        </div>
        {#if ttsStatus}
          <span class="hint">模型存放路径：{ttsStatus.path}</span>
          {#if !ttsStatus.sidecarReady}
            <span class="hint">TTS 组件未安装，请按 tools/build_sidecar.md 构建</span>
          {/if}
        {/if}
        <span class="hint">Kokoro 中文语音合成模型（约 130MB，含 8 个音色），用于会议条目详情的「朗读」。</span>
      </div>
    {:else if section === "pnote"}
      <div class="field">
        <label>开关</label>
        <div class="field-row">
          <label style="display:flex;gap:6px;align-items:center;">
            <input
              type="checkbox"
              bind:checked={form.pnoteEnabled}
              on:change={() => togglePnote(form.pnoteEnabled)}
            />
            启用录音卡同步
          </label>
        </div>
        <span class="hint">启用后立即生效，常驻蓝牙连接；录音保存后自动下载、转写并入库（来源：录音卡）。</span>
      </div>

      <div class="field">
        <label>设备绑定</label>
        <div class="field-row">
          {#if form.pnoteDeviceAddress}
            <span>已绑定 {form.pnoteDeviceAddress}</span>
            <button on:click={unbindPnote} disabled={pnoteBinding}>解绑</button>
          {:else}
            <span>
              未绑定（按广播名匹配）{#if pnoteConnectedAddr}，当前连接 {pnoteConnectedAddr}{:else}，未连接设备{/if}
            </span>
            <button on:click={bindPnote} disabled={pnoteBinding || !pnoteConnectedAddr}>
              绑定当前连接的设备
            </button>
          {/if}
        </div>
        <span class="hint"
          >绑定后只同步这张卡，同名录音卡互不干扰；先在浏览页连接目标录音卡，再回此处点绑定。绑定/解绑立即生效并记入设置。</span
        >
      </div>

      <div class="field">
        <label>本地归档保留天数</label>
        <input type="number" min="1" bind:value={form.pnoteKeepDays} style="width:140px;" />
        <span class="hint">超过天数的 OGG 归档自动删除，已转写入库的条目不受影响。</span>
      </div>

      <div class="field">
        <label>同步时间窗口</label>
        <select bind:value={form.pnoteSyncWindowDays} style="width:140px;">
          <option value={1}>最近 1 天</option>
          <option value={2}>最近 2 天</option>
          <option value={3}>最近 3 天</option>
          <option value={7}>最近 7 天</option>
          <option value={0}>全部</option>
        </select>
        <span class="hint"
          >只同步窗口内开始的录音；窗口外的留在设备上不进库，改宽窗口后可自动捞回。</span
        >
      </div>

      <div class="field">
        <label>洞察分析间隔</label>
        <select bind:value={form.recorderAnalysisIntervalHours} style="width:140px;">
          <option value={0}>手动</option>
          <option value={1}>1 小时</option>
          <option value={2}>2 小时</option>
          <option value={3}>3 小时</option>
          <option value={6}>6 小时</option>
          <option value={12}>12 小时</option>
          <option value={24}>24 小时</option>
        </select>
        <span class="hint">每隔 N 小时自动分析新产生的深度解析；手动模式仅在洞察页点按钮触发。</span>
      </div>

      <div class="field">
        <label>洞察分析提示词</label>
        <textarea bind:value={form.recorderEventPrompt} rows="4"></textarea>
        <span class="hint">分析深度解析提取承诺、灵感、未闭环与项目脉络时作为 system 提示词；输出必须是 JSON。</span>
      </div>

      <div class="field">
        <label>AI 纪要提示词</label>
        <textarea bind:value={form.recorderMinutesPrompt} rows="4" placeholder="留空则使用「会议录音」的纪要提示词"></textarea>
        <span class="hint"
          >录音转写入库后自动生成会议纪要；留空跟随「会议录音」的纪要提示词，填入则独立生效。</span
        >
      </div>

      <div class="field">
        <label>深度解析提示词</label>
        <textarea bind:value={form.recorderDeepPrompt} rows="4" placeholder="留空则使用内置的个人记录导向提示词"></textarea>
        <span class="hint"
          >纪要生成后自动做深度解析；留空使用内置的个人记录导向提示词，填入则自定义。</span
        >
      </div>
    {:else}
      <div class="field">
        <label>全局快捷键</label>
        <input type="text" bind:value={form.hotkey} placeholder="ctrl+alt+space" />
        <span class="hint">修改后立即生效；格式如 ctrl+alt+space、ctrl+shift+f。</span>
      </div>

      <div class="field">
        <label>剪贴板轮询间隔（毫秒）</label>
        <input type="number" min="200" max="5000" bind:value={form.pollIntervalMs} style="width:140px;" />
      </div>

      <div class="field">
        <label>开关</label>
        <div class="field-row">
          <label style="display:flex;gap:6px;align-items:center;">
            <input type="checkbox" bind:checked={form.autostart} />
            开机自启
          </label>
          <label style="display:flex;gap:6px;align-items:center;">
            <input type="checkbox" bind:checked={form.clipboardEnabled} />
            监控剪贴板（关闭后停止自动收集）
          </label>
        </div>
      </div>

      <div class="field">
        <label>排重方式</label>
        <select bind:value={form.dedupScope} style="width:180px;">
          <option value="day">当日排重</option>
          <option value="all">全量排重</option>
        </select>
        <span class="hint">
          当日排重：同一天内相同内容只记录一次，隔天可再次记录；全量排重：相同内容永远只记录一次。
        </span>
      </div>

      <div class="field">
        <label>主题</label>
        <select bind:value={form.theme} style="width:180px;">
          <option value="dark">暗色</option>
          <option value="light">亮色</option>
          <option value="system">跟随系统</option>
        </select>
      </div>
    {/if}
  </div>
</div>

<div class="save-bar">
  <button class="primary" on:click={save} disabled={saving}>{saving ? "保存中…" : "保存设置"}</button>
  <span class="hint">数据库与设置文件位于 %APPDATA%\com.masaifu.app\</span>
</div>
