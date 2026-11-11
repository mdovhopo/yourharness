# Lumen

A tiny habit tracker. No build step, no dependencies: open `index.html` through any static server.

```sh
npm run dev    # http://localhost:5173
npm test
```

Habits and check-ins live in `localStorage` (`src/store.js`); streaks are worked out in `src/streak.js`.
