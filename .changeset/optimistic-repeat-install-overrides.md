---
"@pnpm/config.parse-overrides": patch
"@pnpm/deps.status": patch
"pnpm": patch
"pacquet": patch
---

`optimisticRepeatInstall` keeps the repeat-install fast path when an override replaces a local `file:` dependency with a non-local version [pnpm/pnpm#12892](https://github.com/pnpm/pnpm/issues/12892).
