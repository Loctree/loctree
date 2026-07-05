<script module lang="ts">
  export const prerender = true;
</script>

<script lang="ts">
  import Counter from "$lib/Counter.svelte";
  import { count, increment } from "$lib/store.svelte.ts";
</script>

<svelte:head>
  <title>Svelte 5 fixture</title>
</svelte:head>

<Counter label="Clicks" initial={count} />
<button on:click={increment}>Increment</button>
