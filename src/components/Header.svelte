<script lang="ts">
  import { onMount } from "svelte";
  import { win } from "../lib/api";

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
    <span class="app-subtitle">个人数字分身 v1.2</span>
  </div>

  <button
    class="icon-btn"
    title={maximized ? "还原窗口" : "最大化（不遮挡任务栏）"}
    on:click={toggleMaximize}
  >
    {maximized ? "🗗" : "🗖"}
  </button>
  <button class="icon-btn" title="关闭到托盘（Esc）" on:click={() => win.hide()}>
    ✕
  </button>
</div>
