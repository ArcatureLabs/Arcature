import { vitePreprocess } from '@sveltejs/vite-plugin-svelte'

// No `compilerOptions.runes` here on purpose. That flag reaches dependency
// components too, and `@inertiajs/svelte` still ships Svelte 4 source, which
// fails to compile in forced runes mode. Svelte detects the mode per
// component instead, and every component in this project uses runes.
export default {
  // `<script lang="ts">` is stripped by Vite's own esbuild pass; type errors
  // surface from `npm run check`, never from the dev server.
  preprocess: vitePreprocess(),
}
