import { createInertiaApp, type ResolvedComponent } from '@inertiajs/svelte'
import { mount } from 'svelte'

import '../css/app.css'

// Every page component under `pages/`, loaded on demand. Vite's `glob` is a
// build-time transform, so the bundle only ever contains what exists here --
// a page name the server sends that has no file fails loudly below rather
// than rendering blank.
const pages = import.meta.glob<ResolvedComponent>('./pages/**/*.svelte')

// `id: 'app'` matches the mount point the Inertia adapter emits in the root
// document. Nothing about this file is Arcature-specific: it is the official
// `@inertiajs/svelte` bootstrap, and Arcature publishes no npm package of its
// own to replace it.
void createInertiaApp({
  id: 'app',
  resolve: (name) => {
    const page = pages[`./pages/${name}.svelte`]
    if (!page) {
      throw new Error(`Inertia page "${name}" has no component at pages/${name}.svelte`)
    }
    return page()
  },
  setup({ el, App, props }) {
    if (!el) {
      throw new Error('Inertia mount point #app is missing from the root document')
    }
    mount(App, { target: el, props })
  },
})
