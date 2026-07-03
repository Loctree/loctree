<script lang="ts">
  let { label, initial = 0 } = $props();
  let count = $state(initial);
  const doubled = $derived(count * 2);
  let editable = $bindable(label);
</script>

{#snippet row(value)}
  <span>{value}</span>
{/snippet}

<button>{editable}: {count} / {doubled}</button>
{@render row(count)}
