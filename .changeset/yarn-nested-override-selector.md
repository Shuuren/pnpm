---
"@pnpm/config.parse-overrides": patch
"@pnpm/config.reader": patch
"pnpm": patch
"pacquet": patch
---

`pnpm.overrides` accepts a Yarn selective resolution selector such as `apollo-server-express/**/graphql-tools`. The selector pins that dependency of the named parent.

A `resolutions` field on the root `package.json` is included in `pnpm.overrides` [pnpm/pnpm#5369](https://github.com/pnpm/pnpm/issues/5369).
