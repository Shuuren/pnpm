---
"@pnpm/bins.linker": patch
"@pnpm/installing.linking.hoist": patch
"pnpm": patch
"pacquet": patch
---

`pnpm install` fails when two dependencies would add the same command to one bin directory, unless one package is the only owner of that command. A direct dependency still supplies a command that a hoisted dependency also declares [pnpm/pnpm#7027](https://github.com/pnpm/pnpm/issues/7027).
