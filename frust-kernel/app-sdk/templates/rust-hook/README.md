# {{app_label}}

This project is a Frust app bundle using manifest format 1 and the canonical
Frust plugin WIT contract pinned when the project was created.

```powershell
cargo build --release --target wasm32-wasip2
New-Item -ItemType Directory -Force components | Out-Null
Copy-Item target/wasm32-wasip2/release/{{app_name}}.wasm components/{{app_name}}.wasm
frust-app check
frust-app pack
```

Add the component filename to `components` in `frust-app.json` when a route or
hook needs it. `frust-app check` refuses stale WIT and invalid components before
they reach a kernel.
