---
"@pnpm/engine.pm.commands": patch
"pnpm": patch
"pacquet": patch
---

`pnpm` switches to the version in `packageManager` on Alpine Linux arm64. When the pinned version's native binary cannot be loaded, pnpm runs the JavaScript build with Node.js.

https://github.com/pnpm/pnpm/issues/10443
