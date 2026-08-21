<script setup lang="ts">
import { Head } from '@inertiajs/vue3'

import DefaultLayout from '@/layouts/Default.vue'

/**
 * Props for the `home` page.
 *
 * Declared here so a freshly generated project type-checks and bundles with
 * nothing run first -- `resources/js/generated/` does not exist until
 * `arc typegen` has, and the Dockerfile bundles the frontend in a Node-only
 * stage that has no Rust toolchain to produce it.
 *
 * The authoritative version is generated. Once the project has more than one
 * page, delete this interface and take the props from the graph instead, so
 * that renaming a field in Rust breaks the build rather than the browser:
 *
 * ```ts
 * import type { PropsOf } from '@/generated'
 *
 * defineProps<PropsOf<'home'>>()
 * ```
 */
interface HomeProps {
  message: string
  app_name: string
  arcature_version: string
}

defineProps<HomeProps>()
</script>

<template>
  <DefaultLayout>
    <Head title="Home" />
    <h1>{{ message }}</h1>
    <p class="muted">Running on Arcature {{ arcature_version }}.</p>
    <p>
      Edit <code>resources/js/pages/home.vue</code> and the browser updates
      without a rebuild. Edit <code>app/controllers/home_controller.rs</code>
      and the page reloads once the backend is back up -- the connection is
      never dropped.
    </p>
  </DefaultLayout>
</template>
