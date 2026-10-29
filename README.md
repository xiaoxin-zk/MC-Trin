# MC-Trin

MC-Trin is a Minecraft desktop launcher built on a Tauri runtime with a DeepCraft-inspired interface.

## Features

- Minecraft instance creation, installation, import, launch, and Microsoft device-code login through the reused native runtime.
- CurseForge project search with an API key stored by the native settings layer.
- Built-in MCTier-compatible LAN rooms with public lobby discovery, create/join/cancel/leave lifecycle, and EasyTier v2.6.4 no-TUN TCP port forwarding. Voice chat is not included.
- Modrinth friends and LAN-port sharing use the public Modrinth account and signaling services; users do not need to deploy a friends server.
- An MCP agent that connects to a user-selected MCP server and OpenAI-compatible model endpoint. Model and MCP tokens are session-only; non-read-only tool calls require confirmation.
- Local plugin manifests for themes, static pages, navigation, notifications, and confirmed launch actions. See [MC-TRIN-PLUGIN-API.md](docs/MC-TRIN-PLUGIN-API.md).

## Development

The desktop project requires Node.js 24.15 or newer, pnpm 10.33.2, and the Rust toolchain in `rust-toolchain.toml`.

On Windows, install Visual Studio C++ Build Tools and the MinGW UCRT64 GCC toolchain. The launcher build compiles the Cubiomes seed-map bridge with GCC; set `AXOLOTL_CUBIOMES_CC` when `gcc.exe` is not on `PATH`.

```powershell
pnpm install
pnpm app:dev
```

`pnpm app:build` creates the desktop build. The app UI is in `apps/app-frontend/index.html`; native commands and the launcher runtime remain in `apps/app` and `packages/app-lib`.

## Licensing

The reused launcher source is GPL-3.0-only. Preserve the upstream license and attribution files listed in [MC-TRIN-NOTICES.md](MC-TRIN-NOTICES.md).
