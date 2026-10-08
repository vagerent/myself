<script lang="ts">
  export let source: string;
  export let emoji = "🗂";
</script>

{#if source === "recorder"}
  <svg
    class="source-icon"
    viewBox="0 0 16 16"
    width="14"
    height="14"
    fill="none"
    stroke="currentColor"
    stroke-width="1.2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <rect x="4.5" y="1.5" width="7" height="10.5" rx="2.5" />
    <circle cx="8" cy="5" r="1.15" fill="currentColor" stroke="none" />
    <circle cx="8" cy="13.8" r="1.2" />
  </svg>
{:else}
  {emoji}
{/if}

<style>
  .source-icon {
    display: block;
    color: var(--text-dim);
  }
</style>
