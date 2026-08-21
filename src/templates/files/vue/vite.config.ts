import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// There is no `server` block here, and that is the point. `arc dev` creates
// this config's server itself with `middlewareMode: true` and hands it an IPC
// endpoint (a Unix socket, or a named pipe on Windows), so Vite binds no TCP
// port at all -- the Rust process owns the only one and forwards to it.
// Adding `server.port` here would create the second port this project does
// not have.
export default defineConfig(({ command }) => ({
  // Production assets are served from `/build/...`; the dev server serves
  // source paths from the root, which is where the Rust side looks for them.
  base: command === 'build' ? '/build/' : '/',

  // Vite must not treat `public/` as its own static directory. Its default
  // behaviour is to copy that directory's contents into `outDir` -- and
  // `outDir` here *is* `public/build`, a subdirectory of it, which Vite warns
  // about and which would duplicate `robots.txt` into the hashed-asset tree.
  // Nothing is lost: `public/` is served by the Rust process directly (see
  // `.static_files(..)` in `bootstrap/app.rs`), in development and in
  // production alike, so Vite copying it would only ever produce a second
  // copy of files that were already reachable.
  publicDir: false,

  plugins: [vue()],

  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./resources/js', import.meta.url)),
    },
  },

  build: {
    // The manifest is how the Rust side learns the hashed filenames. Without
    // it every asset reference in production is a guess.
    manifest: true,
    outDir: 'public/build',
    emptyOutDir: true,
    rollupOptions: {
      input: 'resources/js/app.ts',
    },
  },
}))
