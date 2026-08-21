import { createInertiaApp, type ResolvedComponent } from '@inertiajs/react'
import { createRoot } from 'react-dom/client'

import '../css/app.css'

// Every page component under `pages/`, loaded on demand. Vite's `glob` is a
// build-time transform, so the bundle only ever contains what exists here --
// a page name the server sends that has no file fails loudly below rather
// than rendering blank.
const pages = import.meta.glob<{ default: ResolvedComponent }>('./pages/**/*.tsx')

// `id: 'app'` matches the mount point the Inertia adapter emits in the root
// document. Nothing about this file is Arcature-specific: it is the official
// `@inertiajs/react` bootstrap, and Arcature publishes no npm package of its
// own to replace it.
void createInertiaApp({
  id: 'app',
  resolve: (name) => {
    const page = pages[`./pages/${name}.tsx`]
    if (!page) {
      throw new Error(`Inertia page "${name}" has no component at pages/${name}.tsx`)
    }
    // The adapter's `resolve` is typed to return the component, not the
    // module that holds it. Unwrapping `default` here is what the runtime
    // does anyway, and doing it in the type system too means no cast.
    return page().then((module) => module.default)
  },
  setup({ el, App, props }) {
    createRoot(el).render(<App {...props} />)
  },
})
