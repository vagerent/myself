<script lang="ts">
  export let total: number;
  export let page: number;
  export let pageSize: number;

  $: pages = Math.max(1, Math.ceil(total / pageSize));
</script>

{#if pages > 1}
  <div class="pager">
    <button class="icon-btn" disabled={page <= 1} on:click={() => (page -= 1)}>‹ 上一页</button>
    <span class="hint">{page} / {pages}（共 {total} 条）</span>
    <button class="icon-btn" disabled={page >= pages} on:click={() => (page += 1)}>下一页 ›</button>
  </div>
{/if}
