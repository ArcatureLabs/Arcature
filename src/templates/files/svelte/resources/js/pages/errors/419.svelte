<!--
  419 -- the CSRF token did not match.

  In practice this means the session expired while a form sat open. Reloading
  mints a fresh `XSRF-TOKEN` cookie, so the retry succeeds.
-->
<script lang="ts">
  import { Link } from '@inertiajs/svelte'
</script>

<svelte:head>
  <title>Page expired</title>
</svelte:head>

<div class="error-page">
  <p class="status">419</p>
  <h1>The page expired</h1>
  <p class="muted">Your session timed out. Reload and try again.</p>
  <Link class="button" href="/">Go home</Link>
</div>
