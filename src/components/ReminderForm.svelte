<script lang="ts">
  import { api, type ReminderInput } from "../lib/api";
  import { showToast } from "../lib/stores";
  import type { ReminderRow } from "../lib/types";

  export let initial: ReminderRow | null = null;
  export let onSaved: (r: ReminderRow | null) => void;
  export let onCancel: (() => void) | null = null;

  const WEEKDAYS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

  function todayStr(): string {
    const d = new Date();
    const p = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
  }

  let title = initial?.title ?? "";
  let note = initial?.note ?? "";
  let kind: "once" | "daily" | "weekly" = initial?.kind ?? "once";
  let timeOfDay = initial?.timeOfDay ?? "09:00";
  let weekday = initial?.weekday ?? new Date().getDay();
  let fireDate = initial?.fireDate ?? todayStr();
  let saving = false;

  async function save() {
    if (!title.trim()) {
      showToast("请填写标题");
      return;
    }
    const input: ReminderInput = {
      title: title.trim(),
      note: note.trim(),
      kind,
      timeOfDay,
      weekday: kind === "weekly" ? weekday : null,
      fireDate: kind === "once" ? fireDate : null,
    };
    saving = true;
    try {
      if (initial) {
        await api.updateReminder(initial.id, input);
        showToast("提醒已更新");
        onSaved(null);
      } else {
        const row = await api.addReminder(input);
        showToast("提醒已创建");
        onSaved(row);
      }
    } catch (e) {
      showToast(String(e));
    } finally {
      saving = false;
    }
  }
</script>

<div class="settings-form">
  <div class="field">
    <label>标题</label>
    <input type="text" bind:value={title} placeholder="要提醒什么？" />
  </div>

  <div class="field">
    <label>备注（可选）</label>
    <input type="text" bind:value={note} placeholder="补充说明…" />
  </div>

  <div class="field">
    <label>类型</label>
    <select bind:value={kind}>
      <option value="once">单次</option>
      <option value="daily">每天</option>
      <option value="weekly">每周</option>
    </select>
  </div>

  <div class="field">
    <label>时间</label>
    <div class="field-row">
      {#if kind === "once"}
        <input type="date" bind:value={fireDate} />
      {/if}
      {#if kind === "weekly"}
        <select bind:value={weekday}>
          {#each WEEKDAYS as w, i}
            <option value={i}>{w}</option>
          {/each}
        </select>
      {/if}
      <input type="time" bind:value={timeOfDay} />
    </div>
  </div>
</div>

<div class="save-bar">
  <button class="primary" on:click={save} disabled={saving}>
    {saving ? "保存中…" : initial ? "保存修改" : "创建提醒"}
  </button>
  {#if onCancel}
    <button on:click={onCancel}>取消</button>
  {/if}
</div>
