<script setup lang="ts">
import { Head, Link } from '@inertiajs/vue3'
</script>

<template>
  <div class="error-page">
    <Head title="Not found" />
    <p class="status">404</p>
    <h1>Nothing here</h1>
    <p class="muted">That address does not match any route.</p>
    <Link class="button" href="/">Go home</Link>
  </div>
</template>
