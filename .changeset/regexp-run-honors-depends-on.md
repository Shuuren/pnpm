---
"@pnpm/workspace.task-scheduler": patch
"@pnpm/exec.commands": patch
"pnpm": patch
"pacquet": patch
---

`pnpm -r run` with a `/regexp/` script selector now runs each matched script with its `dependsOn` tasks, in graph order. Matched scripts that depend on each other run in that order. A selector used to match no `tasks` entry, so those dependencies were skipped [pnpm/pnpm#15596](https://github.com/pnpm/pnpm/issues/15596).
