<script lang="ts">
  import { api, reminderRuleText, reminderStatus } from "../lib/api";
  import { showToast } from "../lib/stores";
  import type { ReminderRow } from "../lib/types";
  import ReminderForm from "./ReminderForm.svelte";

  export let reminder: ReminderRow;
  export let onChanged: () => void;
  export let onDeleted: () => void;
  export let onBack: () => void;

  let busy = false;

  $: status = reminderStatus(reminder);
  $: terminal = reminder.kind === "once" && !reminder.enabled;

  async function toggleEnabled() {
    busy = true;
    try {
      await api.setReminderEnabled(reminder.id, !reminder.enabled);
      showToast(reminder.enabled ? "已停用" : "已启用");
      onChanged();
    } catch (e) {
      showToast(String(e));
    } finally {
      busy = false;
    }
  }

  async function remove() {
    if (!confirm(`确定删除提醒「${reminder.title}」？`)) return;
    busy = true;
    try {
      await api.deleteReminder(reminder.id);
      showToast("提醒已删除");
      onDeleted();
    } catch (e) {
      showToast(String(e));
    } finally {
      busy = false;
    }
  }
</script>

<div style="padding:14px 22px 0;flex-shrink:0;display:flex;flex-direction:column;gap:8px;">
  <div class="detail-actions" style="align-items:center;">
    <button class="icon-btn" title="返回" on:click={onBack}>←</button>
    <strong style="flex:1;font-size:16px;">编辑任务</strong>
    <span class="badge">{status}</span>
  </div>

  <div class="result-meta">
    <span>{reminderRuleText(reminder)}</span>
    {#if reminder.enabled}
      <span>下次触发：{reminder.nextFireAt.slice(0, 16)}</span>
    {/if}
    {#if reminder.lastFiredAt}
      <span>上次触发：{reminder.lastFiredAt.slice(0, 16)}</span>
    {/if}
  </div>

  <div class="detail-actions">
    {#if !terminal}
      <button on:click={toggleEnabled} disabled={busy}>
        {reminder.enabled ? "停用" : "启用"}
      </button>
    {/if}
    <button on:click={remove} disabled={busy}>删除</button>
  </div>
</div>

<ReminderForm
  initial={reminder}
  onSaved={() => onChanged()}
/>
