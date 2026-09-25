---
"pacquet": patch
---

`pnpm dedupe` produces the same lockfile on every run when a workspace root dependency is an `npm:` alias and a peer of that name comes from `packageExtensions`. Repeated runs previously alternated between that alias and another copy of the peer [#15709](https://github.com/pnpm/pnpm/issues/15709).
