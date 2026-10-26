# MC-Trin Plugin API

MC-Trin accepts a local `manifest.json` from the Plugins page in the main Vue launcher. A manifest can add a sandboxed page or home panel, apply a built-in theme, or declare launcher actions. Installing or enabling a plugin first shows its requested access. Launcher features are exposed through a narrow `window.mctrin` bridge.

Page scripts run in an opaque-origin iframe through a fixed launcher-owned external bootstrap. The host never evaluates plugin code. Script compilation occurs only in the enabled plugin iframe; its CSP permits that bootstrap and blocks fetch, nested frames, workers, and external scripts. A plugin can attempt to navigate its own iframe, so this is not a guarantee of complete network isolation. Leaving the original document revokes the bridge session and removes its iframe. Instance summaries omit paths and credentials. Disable plugins you do not trust.

```json
{
  "id": "example.notes",
  "name": "Example Notes",
  "version": "1.0.0",
  "permissions": ["instances.read", "ui.toast", "app.navigate", "game.launchDefault"],
  "actions": [
    { "id": "saved", "type": "toast", "label": "Show message", "value": "Saved" },
    { "id": "home", "type": "navigate", "label": "Open home", "value": "home" },
    { "id": "launch", "type": "launchDefault", "label": "Launch default instance" }
  ],
  "page": {
    "title": "Notes",
    "html": "<main><h1>My instances</h1><ul id='instances'></ul></main>",
    "script": "window.mctrin.instances.list().then((items) => { const list = document.querySelector('#instances'); for (const item of items) { const row = document.createElement('li'); row.textContent = `${item.name} · ${item.gameVersion}`; list.append(row); } })"
  }
}
```

Supported fields:

- `id`: unique identifier, 3–80 letters, digits, dots, underscores or hyphens.
- `name` and `version`: shown in the plugin manager.
- `theme.accent`: one of `emerald`, `ocean`, `violet`, `amber` or `rose`.
- `theme.mode`: optional `dark`, `light`, or `oled`. Themes apply when the user selects **Apply theme**; installing a plugin does not change the current theme.
- `panels`: up to four objects with `id`, `slot: "home.after"`, `title`, `html`, optional `script`, and an optional `height` between 120 and 600. Each panel renders in its own sandbox below the home dashboard.
- `page.title`, `page.html` and optional `page.script`: a page up to 500 KB and script up to 100 KB.
- `permissions`: `instances.read`, `ui.toast`, `ui.theme`, `app.navigate` and `game.launchDefault`.
- `actions`: up to 12 buttons with `toast`, `theme`, `navigate` or `launchDefault` actions. Each action requires its matching permission. Navigation targets are built-in pages only.

Plugin pages can call `window.mctrin.instances.list()` when they declare `instances.read`. It returns instance ID, name, Minecraft version, loader, and whether that instance is selected. To run a declared action from a page script, call `window.mctrin.run('action-id')`; the promise resolves to `true` or rejects when the action fails, is declined, or lacks permission. Toast, theme, and page navigation actions use their matching permissions. Launch requests require a separate user confirmation.

The iframe has an opaque origin and no filesystem or direct Tauri command access. Plugins do not receive Microsoft credentials, CurseForge keys, MCP tokens, or model API keys. `game.launchDefault` requests launch of the most recently played installed instance and shows its name in a separate confirmation before launch.

The current interface installs local JSON manifests. It does not connect to a remote plugin marketplace.

Plugins are optional and do not interrupt the first-run tour. The manager provides a contextual developer guide, a downloadable example manifest, and a permission review before any newly installed or re-enabled plugin can execute.
