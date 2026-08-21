import { Head } from '@inertiajs/react'

import DefaultLayout from '@/layouts/default'

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
 * export default function Home({ message, arcature_version }: PropsOf<'home'>) {
 * ```
 */
export interface HomeProps {
  message: string
  app_name: string
  arcature_version: string
}

export default function Home({ message, arcature_version }: HomeProps) {
  return (
    <DefaultLayout>
      <Head title="Home" />
      <h1>{message}</h1>
      <p className="muted">Running on Arcature {arcature_version}.</p>
      <p>
        Edit <code>resources/js/pages/home.tsx</code> and the browser updates
        without a rebuild. Edit <code>app/controllers/home_controller.rs</code>{' '}
        and the page reloads once the backend is back up -- the connection is
        never dropped.
      </p>
    </DefaultLayout>
  )
}
