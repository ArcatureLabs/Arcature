import { createInertiaApp } from '@inertiajs/vue3'
import { createApp, h, type DefineComponent } from 'vue'

import '../css/app.css'

// Every page component under `pages/`, loaded on demand. Vite's `glob` is a
// build-time transform, so the bundle only ever contains what exists here --
// a page name the server sends that has no file fails loudly below rather
// than rendering blank.
const pages = import.meta.glob<DefineComponent>('./pages/**/*.vue')

// `id: 'app'` matches the mount point the Inertia adapter emits in the root
// document. Nothing about this file is Arcature-specific: it is the official
// `@inertiajs/vue3` bootstrap, and Arcature publishes no npm package of its
// own to replace it.
void createInertiaApp({
  id: 'app',
  resolve: (name) => {
    const page = pages[`./pages/${name}.vue`]
    if (!page) {
      throw new Error(`Inertia page "${name}" has no component at pages/${name}.vue`)
    }
    return page()
  },
  setup({ el, App, props, plugin }) {
    createApp({ render: () => h(App, props) })
      .use(plugin)
      .mount(el)
  },
})
