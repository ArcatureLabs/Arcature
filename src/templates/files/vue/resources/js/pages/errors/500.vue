<!--
  500 -- the server failed.

  Deliberately says nothing about why. In release builds the framework
  redacts 5xx bodies, and this page must not become the one place a stack
  trace leaks to a browser.
-->
<script setup lang="ts">
import { Head, Link } from '@inertiajs/vue3'
</script>

<template>
  <div class="error-page">
    <Head title="Server error" />
    <p class="status">500</p>
    <h1>Something went wrong</h1>
    <p class="muted">The error has been logged. Try again in a moment.</p>
    <Link class="button" href="/">Go home</Link>
  </div>
</template>
