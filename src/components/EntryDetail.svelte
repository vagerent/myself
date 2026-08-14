<script lang="ts">
  import { api, sourceLabel } from "../lib/api";
  import { showToast } from "../lib/stores";
  import { CATEGORIES, type EntryRow } from "../lib/types";

  export let entry: EntryRow;
  export let onSaved: () => void;
  export let onBack: () => void;

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

  async function save() {
    try {
      await api.updateEntry(entry.id, title, category, tags);
      showToast("已保存");
      onSaved();
    } catch (e) {
      showToast("保存失败：" + String(e));
    }
  }

  async function copy() {
    try {
      await api.copyText(entry.rawContent);
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
  <label style="color:var(--text-dim);font-size:12px;">原始内容</label>
  <textarea readonly value={entry.rawContent} style="flex:1;"></textarea>
  <div class="detail-actions">
    <button class="primary" on:click={save}>保存</button>
    <button on:click={copy}>复制原文</button>
    <button on:click={onBack}>返回</button>
  </div>
</div>
