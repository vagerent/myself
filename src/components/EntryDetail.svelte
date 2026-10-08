<script lang="ts">
  import { api, sourceLabel } from "../lib/api";
  import { showToast } from "../lib/stores";
  import { CATEGORIES, type EntryRow } from "../lib/types";
  import { renderMarkdown } from "../lib/markdown";
  import SpeakBar from "./SpeakBar.svelte";

  export let entry: EntryRow;
  export let onSaved: () => void;
  export let onBack: () => void;
  export let onDeleted: () => void;

  let title = entry.title ?? "";
  let category = entry.category ?? "";
  let showMore = false;
  let tags = "";
  try {
    const parsed = entry.tags ? JSON.parse(entry.tags) : [];
    tags = Array.isArray(parsed) ? parsed.join(", ") : "";
  } catch {
    tags = "";
  }

  // 有 AI 整理内容且与原文不同时，用页签切换「整理内容 / 原始内容」
  // （组件按 entry.id 重建，创建时确定即可）
  const isMeeting = entry.source === "meeting";
  const hasClean = !!entry.cleanContent && entry.cleanContent !== entry.rawContent;
  const cleanLabel = isMeeting ? "会议纪要" : "整理后内容";
  let tab: "clean" | "raw" | "deep" = hasClean ? "clean" : "raw";

  // 深度解析：结果落库，生成过程在后台进行，期间可切换到其他页面
  let deep = entry.deepAnalysis ?? "";
  let deepLoading = false;

  // 朗读面板：点页签行右侧喇叭图标展开
  let showSpeak = false;

  async function generateDeep() {
    if (
      !confirm(
        "深度解析会调用 AI 对会议转写全文做结构化分析（议题、决策、行动项、思维导图），大约需要一两分钟。是否开始？",
      )
    ) {
      return;
    }
    deepLoading = true;
    try {
      deep = await api.generateDeepAnalysis(entry.id);
      // 后端在标题为空时会根据解析结果补题，重新拉取同步过来
      if (!title.trim()) {
        const fresh = await api.getEntry(entry.id).catch(() => null);
        if (fresh?.title) {
          title = fresh.title;
          onSaved();
        }
      }
      showToast("深度解析已生成");
    } catch (e) {
      showToast("深度解析失败：" + String(e));
    } finally {
      deepLoading = false;
    }
  }

  async function save() {
    try {
      await api.updateEntry(entry.id, title, category, tags);
      showToast("已保存");
      onSaved();
    } catch (e) {
      showToast("保存失败：" + String(e));
    }
  }

  async function remove() {
    if (!confirm("确定删除这条记录吗？删除后不可恢复。")) {
      return;
    }
    try {
      await api.deleteEntry(entry.id);
      showToast("已删除");
      onDeleted();
    } catch (e) {
      showToast("删除失败：" + String(e));
    }
  }

  function currentText(): string {
    if (tab === "deep") return deep;
    if (hasClean && tab === "clean") return entry.cleanContent ?? "";
    return entry.rawContent;
  }

  async function copy() {
    try {
      await api.copyText(currentText());
      showToast("已复制到剪贴板");
    } catch (e) {
      showToast("复制失败：" + String(e));
    }
  }
</script>

<div class="detail">
  <div style="display:flex;gap:8px;align-items:center;margin-bottom:4px;">
    <button class="icon-btn" title="返回" on:click={onBack}>←</button>
    <strong>{title || "（无标题）"}</strong>
    <span class="badge {entry.source}">{sourceLabel(entry.source)}</span>
    <span class="badge">{entry.createdAt}</span>
  </div>
  <label style="color:var(--text-dim);font-size:12px;">标题</label>
  <div style="display:flex;gap:8px;align-items:center;">
    <input type="text" style="flex:1;" bind:value={title} placeholder="标题（可留空）" />
    <button type="button" on:click={() => (showMore = !showMore)}>
      {showMore ? "收起 ▴" : "查看更多 ▾"}
    </button>
  </div>
  {#if showMore}
    <div style="display:flex;gap:10px;">
      <div class="field" style="flex:1;margin-bottom:0;">
        <label>分类</label>
        <select bind:value={category}>
          <option value="">（未分类）</option>
          {#each CATEGORIES as c}
            <option value={c}>{c}</option>
          {/each}
        </select>
      </div>
      <div class="field" style="flex:2;margin-bottom:0;">
        <label>标签（逗号分隔）</label>
        <input type="text" bind:value={tags} placeholder="如：工作, 灵感" />
      </div>
    </div>
  {/if}
  {#if hasClean || isMeeting}
    <div class="detail-tabs">
      {#if hasClean}
        <button type="button" class:active={tab === "clean"} on:click={() => (tab = "clean")}>
          {cleanLabel}
        </button>
      {/if}
      <button type="button" class:active={tab === "raw"} on:click={() => (tab = "raw")}>
        原始内容
      </button>
      {#if isMeeting}
        <button type="button" class:active={tab === "deep"} on:click={() => (tab = "deep")}>
          深度解析
        </button>
        <button
          type="button"
          class="speak-toggle"
          style="margin-left:auto;"
          class:active={showSpeak}
          title="朗读"
          on:click={() => (showSpeak = !showSpeak)}
        >
          🔊
        </button>
      {/if}
    </div>
  {/if}
  {#if isMeeting && showSpeak}
    <SpeakBar entryId={entry.id} {tab} />
  {/if}
  {#if tab === "deep" && isMeeting}
    {#if deepLoading}
      <div class="deep-panel">
        <span class="spinner"></span>
        <span class="hint">正在生成深度解析，大约需要一两分钟；期间可以正常切换到其他页面。</span>
      </div>
    {:else if deep}
      <div class="summary-content detail-clean">
        {@html renderMarkdown(deep)}
      </div>
    {:else}
      <div class="deep-panel">
        <span class="hint">
          基于会议转写全文，由 AI 生成决策导向的深度纪要：议题拆解、决策与未决项、行动项清单、业务思维导图。
        </span>
        <button class="primary" on:click={generateDeep}>生成深度解析</button>
      </div>
    {/if}
  {:else if hasClean && tab === "clean"}
    <div class="summary-content detail-clean">
      {@html renderMarkdown(entry.cleanContent ?? "")}
    </div>
  {:else}
    {#if !hasClean && !isMeeting}
      <label style="color:var(--text-dim);font-size:12px;">原始内容</label>
    {/if}
    <textarea readonly value={entry.rawContent} style="flex:1;"></textarea>
  {/if}
  <div class="detail-actions">
    <button class="primary" on:click={save}>保存</button>
    <button on:click={copy} disabled={tab === "deep" && !deep}>
      {#if tab === "deep"}
        复制解析
      {:else if hasClean && tab === "clean"}
        {isMeeting ? "复制纪要" : "复制整理内容"}
      {:else}
        复制原文
      {/if}
    </button>
    <button on:click={onBack}>返回</button>
    <button class="danger" style="margin-left:auto;" on:click={remove}>删除</button>
    {#if isMeeting && tab === "deep" && deep && !deepLoading}
      <button type="button" style="margin-left:auto;" on:click={generateDeep}>重新生成深度解析</button>
    {/if}
  </div>
</div>
