<!--
  500 -- the server failed.

  Deliberately says nothing about why. In release builds the framework redacts
  5xx bodies, and this page must not become the one place a stack trace leaks
  to a browser.
-->
<script lang="ts">
  import { Link } from '@inertiajs/svelte'
</script>

<svelte:head>
  <title>Server error</title>
</svelte:head>

<div class="error-page">
  <p class="status">500</p>
  <h1>Something went wrong</h1>
  <p class="muted">The error has been logged. Try again in a moment.</p>
  <Link class="button" href="/">Go home</Link>
</div>
