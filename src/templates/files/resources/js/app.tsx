// The Inertia client entry point.
// Wire this up to the official @inertiajs/react adapter.

import '../css/app.css';

// Resolve pages by name (e.g. inertia!("home", {}) maps to ./pages/home.tsx).
const pages = import.meta.glob('./pages/**/*.tsx');

export async function resolvePage(name: string) {
  const importer = pages[`./pages/${name}.tsx`];
  if (!importer) {
    throw new Error(`Inertia page "${name}" not found.`);
  }
  return importer();
}
