# MC-Trin Website Scaffold

`apps/website` keeps the Nuxt 4, Vue 3, and Tailwind scaffold for future MC-Trin website work. It is not currently a published site and has no release download service or deployment workflow.

Run `pnpm website:dev` for local development and `pnpm website:build` to generate static output. Follow the [frontend development standard](../../standards/frontend/DEVELOPMENT.md) for implementation and validation.

Keep this package independent from desktop runtime state and Tauri APIs. Add shared UI or service integrations only when a concrete website feature needs them.
