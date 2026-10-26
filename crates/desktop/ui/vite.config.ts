import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

export default defineConfig({
  plugins: [svelte()],
  // Tauri points `devUrl` at a fixed port, so failing loudly on a taken
  // port beats silently moving to one the Rust side isn't watching.
  server: { port: 5173, strictPort: true },
  // Keeps Rust compiler errors on screen instead of wiping them.
  clearScreen: false,
  // The only target is macOS WKWebView (spec §4), so there's no reason
  // to down-level further than this.
  build: { target: 'safari15' },
})
