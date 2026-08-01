# Frust runtime guests

These two crates produce the WebAssembly components required by the kernel:

- `script-engine` is the audited Boa host for metadata scripts.
- `plugin-demo` exercises plugin hooks and routes.

They are runtime source, not disposable spike output. Build them through the
root bootstrap so the kernel component and the Desk browser core are generated
from the same source and checked against `artifacts.lock.json`:

```powershell
pwsh ./scripts/frust.ps1 build-artifacts
```

`artifacts/`, `browser-engine/`, `node_modules/`, and Cargo target directories
are generated and ignored. `artifacts-old-world/` is deliberately tracked: its
binaries are compatibility fixtures proving that the current host still loads
components built against the earlier WIT world.
