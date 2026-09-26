---
"@pnpm/installing.deps-resolver": patch
"pnpm": patch
---

`pnpm install` keeps an unchanged registry tarball's recorded peer dependency ranges in `pnpm-lock.yaml` [pnpm/pnpm#13988](https://github.com/pnpm/pnpm/issues/13988).
