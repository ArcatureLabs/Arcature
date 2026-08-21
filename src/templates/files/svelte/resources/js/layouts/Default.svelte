<!--
  The application shell: navigation and the page body.

  Pages opt in by wrapping their markup in this component, which keeps the
  layout mounted across an Inertia visit -- remount it and any scroll
  position, focus, or component state in the shell is lost on every
  navigation.
-->
<script lang="ts">
  import { Link, usePage } from '@inertiajs/svelte'
  import type { Snippet } from 'svelte'

  import type { LayoutProps } from '@/types'

  let { children }: { children: Snippet } = $props()

  // Not a store, so no `$page`. The Svelte 5 adapter returns a deeply
  // reactive object from runes, and prefixing it reads as a subscription to
  // something with no `subscribe` method -- which `svelte-check` rejects and
  // the compiler cannot make sense of. Reading `page.props` directly in the
  // markup is already reactive.
  const page = usePage<LayoutProps>()
</script>

<nav class="navbar">
  <Link class="navbar-brand" href="/">{page.props.app_name}</Link>
</nav>

<main class="content">{@render children()}</main>
