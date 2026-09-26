import { defineConfig } from 'vitest/config';

export default defineConfig({
    // Keep module ids on the path used to enter the repo (no symlink/junction
    // realpath): running from the C:\NC_work junction makes Vite root a C: path
    // while realpathed module ids resolve to D:/NC_Work/... — jsdom then fetches
    // them via /@fs/ and Vite denies (outside root). preserveSymlinks keeps ids
    // inside root; no effect when running from the physical path (incl. CI).
    resolve: {
        preserveSymlinks: true,
    },
    test: {
        include: ['__tests__/**/*.test.js'],
    },
});
