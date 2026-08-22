import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// base './' so the built app works from the embedded server root.
//
// Stable output filenames on purpose. dist/ is committed (rust_embed bakes it
// into the binary, and neither `cargo install` nor CI can run npm), and the
// repository-wide .gitignore excludes `dist`, so the bundle is force-added. With
// content-hashed names every rebuild would create a NEW ignored path and quietly
// ship a stale console; fixed names keep the tracked paths constant so a rebuild
// is an ordinary diff.
export default defineConfig({
  plugins: [react()],
  base: './',
  build: {
    outDir: 'dist',
    rollupOptions: {
      output: {
        entryFileNames: 'assets/index.js',
        chunkFileNames: 'assets/[name].js',
        assetFileNames: 'assets/[name].[ext]',
      },
    },
  },
});
