<script lang="ts">
  import { Link } from '@inertiajs/svelte'
</script>

<svelte:head>
  <title>Not found</title>
</svelte:head>

<div class="error-page">
  <p class="status">404</p>
  <h1>Nothing here</h1>
  <p class="muted">That address does not match any route.</p>
  <Link class="button" href="/">Go home</Link>
</div>
