<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { api } from "../lib/api";
  import { loadSettings, showToast, settings, view } from "../lib/stores";
  import type { AsrModelStatus, Settings } from "../lib/types";

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
  };
  let saving = false;
  let asrStatus: AsrModelStatus | null = null;
  let downloadingModel = false;
  let downloadText = "";

  // 设置分类页签
  const sections = [
    { id: "ai", label: "AI 服务" },
    { id: "summary", label: "汇总" },
    { id: "meeting", label: "会议录音" },
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

  onMount(() => {
    refreshAsrStatus();
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
    return () => {
      unlistenDownload.then((f) => f());
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
