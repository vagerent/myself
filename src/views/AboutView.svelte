<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { api } from "../lib/api";
  import { showToast, view } from "../lib/stores";

  let version = "";

  onMount(async () => {
    version = await getVersion().catch(() => "");
  });
</script>

<div class="about">
  <div class="logo">马赛福<span class="dot-accent">.</span></div>
  <div class="slogan">个人信息搜索和蒸馏工具</div>
  <span class="badge version">v{version || "1.0.0"}</span>

  <p class="desc">
    常驻系统托盘的个人信息工具：自动收集剪贴板内容与手动记录的灵感，
    经 AI 蒸馏为分类条目，支持全文搜索、每日 / 每周汇总和任务提醒。
  </p>

  <div class="meta">
    <div class="meta-row">
      <span class="k">官网</span>
      <button
        type="button"
        class="link-btn"
        title="在浏览器中打开"
        on:click={() => api.openUrl("https://myself.mavenzero.com/").catch((e) => showToast(String(e)))}
      >
        myself.mavenzero.com
      </button>
    </div>
    <div class="meta-row">
      <span class="k">技术栈</span>
      <span>Tauri 2 · Svelte · SQLite</span>
    </div>
    <div class="meta-row">
      <span class="k">数据目录</span>
      <span>%APPDATA%\com.masaifu.desktop</span>
    </div>
    <div class="meta-row">
      <span class="k">汇总目录</span>
      <span>文档\Masaifu（可在设置中修改）</span>
    </div>
  </div>

  <button on:click={() => view.set("browse")}>← 返回分类浏览</button>
</div>

<style>
  .about {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 24px;
    overflow: auto;
    text-align: center;
  }

  .logo {
    font-size: 34px;
    font-weight: 700;
    letter-spacing: 2px;
  }

  .dot-accent {
    color: var(--accent);
  }

  .slogan {
    color: var(--text-dim);
    font-size: 13px;
  }

  .version {
    font-variant-numeric: tabular-nums;
  }

  .desc {
    max-width: 46ch;
    color: var(--text-dim);
    font-size: 13px;
    line-height: 1.8;
    margin: 8px 0 4px;
  }

  .meta {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 6px 0 14px;
    padding: 12px 18px;
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    font-size: 12px;
    text-align: left;
  }

  .meta-row {
    display: flex;
    gap: 12px;
  }

  .meta-row .k {
    color: var(--text-dim);
    flex-shrink: 0;
    width: 60px;
  }

  .link-btn {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    font-size: 12px;
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 3px;
  }

  .link-btn:hover {
    opacity: 0.85;
  }
</style>
