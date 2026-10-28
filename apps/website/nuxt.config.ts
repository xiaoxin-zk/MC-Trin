export default defineNuxtConfig({
    srcDir: 'src/',
    dir: {
        public: 'public',
    },
    app: {
        head: {
            htmlAttrs: { lang: 'zh-CN' },
            title: 'MC-Trin',
        },
    },
    experimental: {
        appManifest: false,
    },
    css: ['~/assets/styles/tailwind.css'],
    postcss: {
        plugins: {
            tailwindcss: {},
            autoprefixer: {},
        },
    },
    typescript: {
        shim: false,
        strict: true,
        typeCheck: false,
    },
    compatibilityDate: '2025-01-01',
    telemetry: false,
})
