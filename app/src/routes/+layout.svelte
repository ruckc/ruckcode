<script lang="ts">
  import '../app.css';

  let { children } = $props();
</script>

<svelte:head>
  <title>ruckcode</title>
  <link rel="icon" href="/favicon.svg" />
</svelte:head>

{@render children()}
