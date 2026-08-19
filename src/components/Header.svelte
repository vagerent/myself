<script lang="ts">
  import { onMount } from "svelte";
  import { win } from "../lib/api";

  export let compact: boolean;
  export let onToggleMode: () => void;

  let maximized = false;

  onMount(async () => {
    maximized = await win.isMaximized().catch(() => false);
  });

  async function toggleMaximize() {
    const next = !maximized;
    try {
      if (next) {
        await win.maximize();
      } else {
        await win.unmaximize();
      }
      maximized = next;
    } catch {
      // 忽略：窗口状态不支持时静默失败
    }
  }
</script>

<div class="header">
  <div class="drag" data-tauri-drag-region>
    <span class="app-title">马赛福</span>
    <span class="app-subtitle">个人信息搜索和蒸馏工具 v1.1</span>
  </div>

  {#if !compact}
    <button
      class="icon-btn"
      title="还原（精简搜索）"
      on:click={onToggleMode}
    >
      🗕
    </button>
    <button
      class="icon-btn"
      title={maximized ? "还原窗口" : "最大化（不遮挡任务栏）"}
      on:click={toggleMaximize}
    >
      {maximized ? "🗗" : "🗖"}
    </button>
  {/if}
  {#if compact}
    <button
      class="icon-btn"
      title="最大化（分类浏览）"
      on:click={onToggleMode}
    >
      ⛶
    </button>
  {/if}
  <button class="icon-btn" title="关闭到托盘（Esc）" on:click={() => win.hide()}>
    ✕
  </button>
</div>
