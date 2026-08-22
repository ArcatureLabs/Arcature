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
    <!--
      `href="/"` rather than `route('home')`. The typed helper is generated
      into `resources/js/generated/`, which is derived from the Rust graph and
      never committed, so importing it from the scaffold would leave a fresh
      clone with an unresolved import on line one.

      Switch to `route('home')` once the project is yours -- `just check-ts`
      runs `arc typegen` before `tsc`, so route names are checked against the
      live graph and renaming one in Rust turns every stale call site red.
      Arcature ADR 0006, at
      https://arcaturelabs.github.io/Arcature/decisions.html, records why
      it is not the default.
    -->
    <Link class="navbar-brand" href="/">{{ app_name }}</Link>
  </nav>

  <main class="content">
    <slot />
  </main>
</template>
