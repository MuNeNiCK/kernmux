import { resolve } from "node:path"
import tailwindcss from "@tailwindcss/vite"
import { defineConfig } from "vite"
import solid from "vite-plugin-solid"

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  resolve: { alias: { "@": resolve(import.meta.dirname, "src") } },
  build: { outDir: "../dist/web", emptyOutDir: true },
  server: { proxy: { "/api": { target: "http://127.0.0.1:9443", ws: true } } },
})
