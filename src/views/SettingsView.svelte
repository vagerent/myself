<script lang="ts">
  import { api } from "../lib/api";
  import { loadSettings, showToast, settings, view } from "../lib/stores";
  import type { Settings } from "../lib/types";

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
  };
  let saving = false;

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
</script>

<div
  style="display:flex;align-items:center;gap:10px;padding:10px 22px;border-bottom:1px solid var(--border);flex-shrink:0;"
>
  <button on:click={() => view.set("browse")}>← 返回分类浏览</button>
  <strong>设置</strong>
</div>

<div class="settings-form">
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
    <div class="field">
      <label>剪贴板轮询间隔（毫秒）</label>
      <input type="number" min="200" max="5000" bind:value={form.pollIntervalMs} style="width:140px;" />
    </div>
  </div>

  <div class="field">
    <label>Markdown 汇总输出目录</label>
    <input type="text" bind:value={form.markdownDir} placeholder="留空则使用 文档\Masaifu\daily" />
  </div>

  <div class="field">
    <label>全局快捷键</label>
    <input type="text" bind:value={form.hotkey} placeholder="ctrl+alt+space" />
    <span class="hint">修改后立即生效；格式如 ctrl+alt+space、ctrl+shift+f。</span>
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

  <div class="field">
    <label>开关</label>
    <div class="field-row">
      <label style="display:flex;gap:6px;align-items:center;">
        <input type="checkbox" bind:checked={form.aiAnswerEnabled} />
        AI 回答（搜索时基于结果回答）
      </label>
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
</div>

<div class="save-bar">
  <button class="primary" on:click={save} disabled={saving}>{saving ? "保存中…" : "保存设置"}</button>
  <span class="hint">数据库与设置文件位于 %APPDATA%\com.masaifu.app\</span>
</div>
