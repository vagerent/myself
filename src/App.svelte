<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import Header from "./components/Header.svelte";
  import BrowseView from "./views/BrowseView.svelte";
  import SettingsView from "./views/SettingsView.svelte";
  import AboutView from "./views/AboutView.svelte";
  import {
    api,
    getCachedWindowState,
    loadWindowState,
    setCachedWindowState,
    setFullMode,
    win,
  } from "./lib/api";
  import { loadSettings, pendingReminderFocus, toast, view } from "./lib/stores";
  import { watchSystemTheme } from "./lib/theme";

  type AppAlert = { heading: string; title: string; note: string };
  let appAlerts: AppAlert[] = [];
  $: currentAlert = appAlerts.length > 0 ? appAlerts[0] : null;

  function openView(v: "browse" | "settings" | "about") {
    view.set(v);
    setFullMode();
  }

  // 窗口大小/位置变化后防抖保存，下次启动恢复
  let saveTimer: ReturnType<typeof setTimeout> | null = null;

  function scheduleSaveWindowState() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(persistWindowState, 400);
  }

  async function persistWindowState() {
    if (await win.isMaximized().catch(() => false)) return;
    // 最小化时系统报告的位置/尺寸是屏幕外占位值（如 -32000），不能入库
    if (await win.isMinimized().catch(() => false)) return;
    const size = await win.innerSize();
    const pos = await win.outerPosition();
    if (pos.x < -10000 || pos.y < -10000) return;
    const s = { ...getCachedWindowState() };
    s.fullWidth = size.width;
    s.fullHeight = size.height;
    s.x = pos.x;
    s.y = pos.y;
    setCachedWindowState(s);
    await api.saveWindowState(s).catch(() => {});
  }

  onMount(() => {
    loadSettings();
    loadWindowState();
    watchSystemTheme();
    const unResized = win.onResized(scheduleSaveWindowState);
    const unMoved = win.onMoved(scheduleSaveWindowState);
    const un1 = listen("focus-search", () => {
      openView("browse");
    });
    const un2 = listen("show-settings", () => {
      openView("settings");
    });
    const un3 = listen("show-summary", () => {
      openView("browse");
    });
    const un4 = listen("show-reminders", () => {
      pendingReminderFocus.set(true);
      openView("browse");
    });
    const un5 = listen("show-about", () => {
      openView("about");
    });
    const un6 = listen<AppAlert>("app-alert", (e) => {
      appAlerts = [...appAlerts, e.payload];
    });
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (currentAlert) {
          appAlerts = appAlerts.slice(1);
        } else {
          win.hide();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      if (saveTimer) clearTimeout(saveTimer);
      unResized.then((f) => f());
      unMoved.then((f) => f());
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
      un4.then((f) => f());
      un5.then((f) => f());
      un6.then((f) => f());
      window.removeEventListener("keydown", onKey);
    };
  });
</script>

<div class="app-shell">
  <Header />

  <div class="content">
    {#if $view === "browse"}
      <BrowseView />
    {:else if $view === "about"}
      <AboutView />
    {:else}
      <SettingsView />
    {/if}
  </div>
</div>

{#if $toast}
  <div class="toast">{$toast}</div>
{/if}

{#if currentAlert}
  <div class="alert-overlay">
    <div class="alert-card">
      <div class="alert-heading">🔔 {currentAlert.heading}</div>
      <div class="alert-title">{currentAlert.title}</div>
      {#if currentAlert.note}
        <div class="alert-note">{currentAlert.note}</div>
      {/if}
      <button class="alert-btn" on:click={() => (appAlerts = appAlerts.slice(1))}>
        知道了{#if appAlerts.length > 1}（还有 {appAlerts.length - 1} 条）{/if}
      </button>
    </div>
  </div>
{/if}
