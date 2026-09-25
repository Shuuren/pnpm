---
"pacquet": patch
---

A global bin shim such as `node` runs when pnpm itself is a relative symlink. `pnpm runtime set`, `pnpm shim add`, and `pnpm self-update` publish a hard link or a copy of the pnpm binary. A Homebrew install used to leave that shim dangling from the global bin directory.

Closes pnpm/pnpm#15691
