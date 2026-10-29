import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
    },
  },
  server: {
    proxy: {
      '/relay': {
        // The fork's relay is served at the origin root, so the local `/relay`
        // prefix has to be stripped before the request reaches it.
        target: 'https://obf.test.linkerpi.com',
        changeOrigin: true,
        secure: true,
        ws: true,
        rewrite: (pathname) => pathname.replace(/^\/relay/, ''),
      },
    },
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
  },
  base: './',
});
