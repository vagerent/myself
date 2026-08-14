<script lang="ts">
  import { api, clipText, sourceLabel } from "../lib/api";
  import { onMount } from "svelte";
  import { showToast } from "../lib/stores";
  import type { EntryRow, SearchHit } from "../lib/types";
  import { CATEGORIES } from "../lib/types";

  let query = "";
  let hits: SearchHit[] = [];
  let searching = false;
  let selected: SearchHit | null = null;
  let detailEntry: EntryRow | null = null;
  let answering = false;
  let aiSearching = false;
  let aiAnswer = "";
  let manualText = "";
  let debounce: ReturnType<typeof setTimeout> | null = null;
  let searchInput: HTMLInputElement;

  let editTitle = "";
  let editCategory = "";
  let editTags = "";

  async function doSearch() {
    if (!query.trim()) {
      hits = [];
      return;
    }
    searching = true;
    try {
      hits = await api.search(query, 50);
    } catch (e) {
      showToast("搜索失败：" + String(e));
    } finally {
      searching = false;
    }
    // 直接搜索有结果则不启动 AI 流程；无结果时由 AI 提取搜索词重新搜索
    if (hits.length === 0 && query.trim()) {
      await aiFallbackSearch(query.trim());
    }
  }

  // AI 兜底搜索：提取搜索词 → 每个词单独搜索取前 5 条 → 排重 → 基于结果生成 AI 回答
  async function aiFallbackSearch(q: string) {
    aiSearching = true;
    try {
      const terms = await api.aiSearchTerms(q);
      if (query.trim() !== q) return;
      const seen = new Set<string>();
      const merged: SearchHit[] = [];
      for (const t of terms) {
        const r = await api.search(t, 5);
        for (const h of r) {
          const k = h.docType + h.docId;
          if (!seen.has(k)) {
            seen.add(k);
            merged.push(h);
          }
        }
      }
      if (query.trim() !== q) return;
      hits = merged;
      if (merged.length > 0) {
        answering = true;
        try {
          aiAnswer = await api.aiAnswer(q, merged.slice(0, 10));
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

  function onInput() {
    aiAnswer = "";
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(doSearch, 250);
  }

  async function askAi() {
    if (!query.trim()) return;
    if (hits.length === 0) {
      showToast("没有可提供给 AI 的搜索结果");
      return;
    }
    answering = true;
    aiAnswer = "";
    try {
      aiAnswer = await api.aiAnswer(query, hits.slice(0, 10));
    } catch (e) {
      aiAnswer = "";
      showToast("AI 回答失败：" + String(e));
    } finally {
      answering = false;
    }
  }

  async function openDetail(hit: SearchHit) {
    selected = hit;
    detailEntry = null;
    if (hit.docType === "entry") {
      try {
        detailEntry = await api.getEntry(Number(hit.docId));
        editTitle = detailEntry.title ?? "";
        editCategory = detailEntry.category ?? "";
        editTags = detailEntry.tags ? JSON.parse(detailEntry.tags).join(", ") : "";
      } catch (e) {
        showToast("读取详情失败：" + String(e));
      }
    }
  }

  function backToList() {
    selected = null;
    detailEntry = null;
  }

  async function saveDetail() {
    if (!detailEntry) return;
    try {
      await api.updateEntry(detailEntry.id, editTitle, editCategory, editTags);
      showToast("已保存");
      backToList();
      doSearch();
    } catch (e) {
      showToast("保存失败：" + String(e));
    }
  }

  async function copyEntry() {
    if (!detailEntry) return;
    try {
      await api.copyText(detailEntry.rawContent);
      showToast("已复制到剪贴板");
    } catch (e) {
      showToast("复制失败：" + String(e));
    }
  }

  async function addManual() {
    const text = manualText.trim();
    if (!text) return;
    try {
      await api.addEntry(text);
      manualText = "";
      showToast("已记入灵感");
      doSearch();
    } catch (e) {
      showToast("记录失败：" + String(e));
    }
  }

  function highlight(text: string): string {
    const q = query.trim();
    if (!q) return text;
    const esc = (s: string) =>
      s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const escaped = esc(text);
    const re = new RegExp(`(${q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`, "gi");
    return escaped.replace(re, '<span class="mark">$1</span>');
  }

  onMount(() => {
    if (searchInput) searchInput.focus();
  });
</script>

{#if selected && detailEntry}
  <div class="detail">
    <div style="display:flex;gap:8px;align-items:center;margin-bottom:4px;">
      <button class="icon-btn" title="返回" on:click={backToList}>←</button>
      <strong>{editTitle || "（无标题）"}</strong>
      <span class="badge {detailEntry.source}">{sourceLabel(detailEntry.source)}</span>
      <span class="badge">{detailEntry.createdAt}</span>
    </div>
    <label style="color:var(--text-dim);font-size:12px;">标题</label>
    <input type="text" bind:value={editTitle} placeholder="标题（可留空）" />
    <div style="display:flex;gap:10px;">
      <div class="field" style="flex:1;margin-bottom:0;">
        <label>分类</label>
        <select bind:value={editCategory}>
          <option value="">（未分类）</option>
          {#each CATEGORIES as c}
            <option value={c}>{c}</option>
          {/each}
        </select>
      </div>
      <div class="field" style="flex:2;margin-bottom:0;">
        <label>标签（逗号分隔）</label>
        <input type="text" bind:value={editTags} placeholder="如：工作, 灵感" />
      </div>
    </div>
    <label style="color:var(--text-dim);font-size:12px;">原始内容</label>
    <textarea readonly value={detailEntry.rawContent} style="flex:1;"></textarea>
    <div class="detail-actions">
      <button class="primary" on:click={saveDetail}>保存</button>
      <button on:click={copyEntry}>复制原文</button>
      <button on:click={backToList}>返回</button>
    </div>
  </div>
{:else}
  <div class="search-input-row">
    <input
      type="text"
      placeholder="搜索剪贴板、灵感与每日汇总…"
      bind:value={query}
      on:input={onInput}
      on:keydown={(e) => {
        if (e.key === "Enter") askAi();
      }}
      bind:this={searchInput}
    />
    <button
      class="primary"
      on:click={askAi}
      disabled={answering || !query.trim() || hits.length === 0}
    >
      {answering ? "思考中…" : "AI 回答"}
    </button>
  </div>

  {#if aiSearching}
    <div class="ai-answer">
      <span class="spinner"></span>直接搜索未命中，AI 正在提取关键词重新搜索…
    </div>
  {:else if answering}
    <div class="ai-answer">
      <span class="spinner"></span>正在基于前 10 条结果生成回答…
    </div>
  {:else if aiAnswer}
    <div class="ai-answer">{aiAnswer}</div>
  {/if}

  <div class="results">
    {#if searching}
      <div class="loading"><span class="spinner"></span>搜索中…</div>
    {:else if aiSearching}
      <div class="loading"><span class="spinner"></span>AI 搜索中…</div>
    {:else if hits.length === 0}
      <div class="empty">
        {query ? "没有找到匹配内容" : "输入关键词开始搜索"}
      </div>
    {:else}
      {#each hits as hit (hit.docType + hit.docId)}
        <button type="button" class="result-item" on:click={() => openDetail(hit)}>
          <div class="result-title">
            <span class="badge {hit.source}">{sourceLabel(hit.source)}</span>
            <span>{hit.title}</span>
            {#if hit.category}
              <span class="badge">{hit.category}</span>
            {/if}
          </div>
          <div class="result-meta">
            <span>{hit.createdAt.slice(0, 16)}</span>
            {#if hit.tags}
              <span>{hit.tags}</span>
            {/if}
          </div>
          <div class="result-preview" style="user-select:text;">
            {@html highlight(clipText(hit.content, 200))}
          </div>
        </button>
      {/each}
    {/if}
  </div>

  <div class="manual-input">
    <textarea
      placeholder="记录一条灵感…（Ctrl+Enter 保存）"
      bind:value={manualText}
      on:keydown={(e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          addManual();
        }
      }}
    ></textarea>
    <button class="primary" on:click={addManual} disabled={!manualText.trim()}>
      记入
    </button>
  </div>
{/if}
