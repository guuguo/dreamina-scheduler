import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: 4186,
    strictPort: true,
    watch: {
      ignored: [
        '**/.antigravity/**',
        '**/.claude/**',
        '**/.codegraph/**',
        '**/.codegraphcontext/**',
        '**/.git/**',
        '**/.omo/**',
        '**/.serena/**',
        '**/.tmp/**',
        '**/dist/**',
        '**/docs/**',
        '**/src-tauri/target/**',
      ],
    },
  },
  clearScreen: false,
});
