import { defineMessages } from '@modrinth/ui'

export const pluginMessages = defineMessages({
    title: { id: 'app.plugins.title', defaultMessage: 'Plugins' },
    description: {
        id: 'app.plugins.description',
        defaultMessage: 'Add local pages, home panels, launcher actions, and themes.',
    },
    install: { id: 'app.plugins.install', defaultMessage: 'Install local plugin' },
    file: { id: 'app.plugins.file', defaultMessage: 'Choose a plugin manifest' },
    docs: { id: 'app.plugins.docs', defaultMessage: 'Developer guide' },
    sample: { id: 'app.plugins.sample', defaultMessage: 'Download example manifest' },
    empty: {
        id: 'app.plugins.empty',
        defaultMessage: 'Choose a local manifest.json to install a plugin.',
    },
    enabled: { id: 'app.plugins.enabled', defaultMessage: 'Enabled' },
    disabled: { id: 'app.plugins.disabled', defaultMessage: 'Disabled' },
    enable: { id: 'app.plugins.enable', defaultMessage: 'Enable' },
    disable: { id: 'app.plugins.disable', defaultMessage: 'Disable' },
    remove: { id: 'app.plugins.remove', defaultMessage: 'Uninstall' },
    open: { id: 'app.plugins.open', defaultMessage: 'Open page' },
    close: { id: 'app.plugins.close', defaultMessage: 'Close page' },
    cancel: { id: 'app.plugins.cancel', defaultMessage: 'Cancel' },
    theme: { id: 'app.plugins.theme', defaultMessage: 'Apply theme' },
    panels: { id: 'app.plugins.panels', defaultMessage: 'Home panels: {count}' },
    permissions: { id: 'app.plugins.permissions', defaultMessage: 'Requested access' },
    noPermissions: {
        id: 'app.plugins.no-permissions',
        defaultMessage: 'No launcher access requested',
    },
    review: { id: 'app.plugins.review', defaultMessage: 'Review plugin' },
    reviewHint: {
        id: 'app.plugins.review-hint',
        defaultMessage:
            'Enable plugins from authors you trust. Review their requested access before installing.',
    },
    confirmInstall: { id: 'app.plugins.confirm-install', defaultMessage: 'Install and enable' },
    confirmReplace: { id: 'app.plugins.confirm-replace', defaultMessage: 'Replace and enable' },
    reviewEnable: { id: 'app.plugins.review-enable', defaultMessage: 'Enable plugin' },
    back: { id: 'app.plugins.back', defaultMessage: 'Back to plugins' },
    installed: { id: 'app.plugins.installed', defaultMessage: 'Plugin installed: {name}' },
    themeApplied: { id: 'app.plugins.theme-applied', defaultMessage: 'Plugin theme applied.' },
    removeTitle: { id: 'app.plugins.remove-title', defaultMessage: 'Uninstall plugin?' },
    removeConfirm: {
        id: 'app.plugins.remove-confirm',
        defaultMessage: 'Uninstall {name} and remove its local manifest?',
    },
    guideTitle: { id: 'app.plugins.guide-title', defaultMessage: 'Build a local plugin' },
    guide: {
        id: 'app.plugins.guide',
        defaultMessage:
            'Create a JSON manifest with an id, name, version, permissions, and a page, action, theme, or home panel. Use window.mctrin.instances.list() to read instance summaries and window.mctrin.run(actionId) to run a declared action. Plugins cannot read files, access credentials, or call native commands.',
    },
    page: { id: 'app.plugins.feature.page', defaultMessage: 'Plugin page' },
    actions: { id: 'app.plugins.feature.actions', defaultMessage: 'Actions: {count}' },
    themeFeature: { id: 'app.plugins.feature.theme', defaultMessage: 'Launcher theme' },
    instancesPermission: {
        id: 'app.plugins.permission.instances-read',
        defaultMessage: 'Read instance names and versions',
    },
    toastPermission: {
        id: 'app.plugins.permission.ui-toast',
        defaultMessage: 'Show launcher messages',
    },
    themePermission: {
        id: 'app.plugins.permission.ui-theme',
        defaultMessage: 'Change launcher theme',
    },
    navigatePermission: {
        id: 'app.plugins.permission.app-navigate',
        defaultMessage: 'Open built-in launcher pages',
    },
    launchPermission: {
        id: 'app.plugins.permission.game-launch-default',
        defaultMessage: 'Request a game launch after confirmation',
    },
})

export const exampleLocalPlugin = {
    id: 'example.instances',
    name: 'Instance Notes',
    version: '1.0.0',
    description: 'A local instance overview with a home panel and optional launcher theme.',
    permissions: ['instances.read', 'ui.toast', 'ui.theme', 'app.navigate'],
    theme: { accent: 'ocean', mode: 'dark' },
    actions: [
        { id: 'home', label: 'Open home', type: 'navigate', value: 'home' },
        { id: 'hello', label: 'Show message', type: 'toast', value: 'Hello from Instance Notes' },
    ],
    page: {
        title: 'Instance Notes',
        html: '<main><h1>My instances</h1><ul id="instances"></ul><button id="home">Open home</button></main>',
        script: 'window.mctrin.instances.list().then((items) => { const list = document.querySelector("#instances"); for (const item of items) { const row = document.createElement("li"); row.textContent = `${item.name} · ${item.gameVersion}`; list.append(row); } }); document.querySelector("#home").addEventListener("click", () => window.mctrin.run("home"));',
    },
    panels: [
        {
            id: 'notes',
            slot: 'home.after',
            title: 'Instance Notes',
            height: 160,
            html: '<p>Welcome to your local plugin panel.</p><button id="hello">Show message</button>',
            script: 'document.querySelector("#hello").addEventListener("click", () => window.mctrin.run("hello"));',
        },
    ],
}
