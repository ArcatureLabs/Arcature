<!--
  The application shell: navigation and the page body.

  Pages opt in by wrapping their template in this component, which keeps the
  layout mounted across an Inertia visit -- remount it and any scroll
  position, focus, or component state in the shell is lost on every
  navigation.
-->
<script setup lang="ts">
import { Link, usePage } from '@inertiajs/vue3'
import { computed } from 'vue'

import type { LayoutProps } from '@/types'

const page = usePage<LayoutProps>()
const app_name = computed(() => page.props.app_name)
</script>

<template>
  <nav class="navbar">
    <Link class="navbar-brand" href="/">{{ app_name }}</Link>
  </nav>

  <main class="content">
    <slot />
  </main>
</template>
