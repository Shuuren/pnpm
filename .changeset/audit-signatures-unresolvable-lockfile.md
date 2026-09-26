---
"@pnpm/deps.compliance.audit": patch
"@pnpm/deps.compliance.commands": patch
"pacquet": patch
"pnpm": patch
---

`pnpm audit signatures` now reports a dependency whose lockfile reference has no matching entry as invalid and exits with a failure. That dependency stays in the audited count [#13638](https://github.com/pnpm/pnpm/issues/13638).
