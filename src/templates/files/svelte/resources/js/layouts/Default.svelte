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

<!--
  `href="/"` rather than `route('home')`. The typed helper is generated into
  `resources/js/generated/`, which is derived from the Rust graph and never
  committed, so importing it from the scaffold would leave a fresh clone with
  an unresolved import on line one.

  Switch to `route('home')` once the project is yours -- `just check-ts` runs
  `arc typegen` before `tsc`, so route names are checked against the live
  graph and renaming one in Rust turns every stale call site red. Arcature
  ADR 0006, at https://arcaturelabs.github.io/Arcature/decisions.html,
  records why it is not the default.
-->
<nav class="navbar">
  <Link class="navbar-brand" href="/">{page.props.app_name}</Link>
</nav>

<main class="content">{@render children()}</main>
