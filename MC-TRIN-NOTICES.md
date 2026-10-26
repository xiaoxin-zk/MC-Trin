# MC-Trin Notices

MC-Trin reuses the Axolotl Launcher source tree as its desktop runtime. The reused launcher source is licensed under GNU GPL version 3 only. Preserve the upstream `apps/app/LICENSE`, `apps/app/COPYING.md`, `apps/app-frontend/LICENSE`, and `apps/app-frontend/COPYING.md` when modifying or distributing this project.

The DeepCraft layout supplied in `D:/mc/ui模板/deepseek` is retained as a UI prototype at `apps/app-frontend/mctrin-prototype.html`. The desktop entry uses the Axolotl Vue runtime and its existing native launcher workflows, with MC-Trin branding. MCTier is used as a protocol and feature reference; its application source is not included in this project.

MC-Trin is an independent community project and is not affiliated with Mojang, Microsoft, CurseForge, Modrinth, Axolotl, or MCTier.

## EasyTier runtime

The optional MC-Trin LAN provider downloads the official EasyTier v2.6.4
release archive at runtime and verifies its SHA-256 digest before extraction.
The provider uses only the `easytier-core` process in no-TUN mode with TCP port
forwarding; it does not install or modify network drivers. EasyTier is licensed
under LGPL-3.0. The source and the exact release archives are available at
https://github.com/EasyTier/EasyTier/releases/tag/v2.6.4. The platform archive
digests are pinned in `packages/app-lib/src/api/mctier/binary.rs`.

The MC-Trin MCTier provider independently implements the public v3 signaling
wire format. It does not include MCTier application source code. The provider's
compatibility level is sent separately from MC-Trin's application identity.
