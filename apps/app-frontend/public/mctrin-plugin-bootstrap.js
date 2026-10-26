/* Fixed launcher-owned bootstrap. This file runs only inside an opaque sandbox iframe. */
(() => {
    'use strict';
    if (window === window.parent || window.origin !== 'null') return;
    const session = document.querySelector('meta[name="mctrin-session"]')?.content;
    if (!/^[a-z0-9_-]{16,80}$/i.test(session || '')) return;
    let sequence = 0;
    let closed = false;
    const pending = new Map();
    const notify = (type, value) => parent.postMessage({ type, session, ...value }, '*');
    const close = () => {
        if (closed) return;
        closed = true;
        for (const request of pending.values()) {
            window.clearTimeout(request.timer);
            request.reject(new Error('Plugin document closed'));
        }
        pending.clear();
        notify('mctrin:closing', {});
    };
    window.addEventListener('pagehide', close, { once: true });
    window.addEventListener('beforeunload', close, { once: true });
    window.addEventListener('message', (event) => {
        if (closed || event.source !== parent || event.data?.type !== 'mctrin:response' || event.data.session !== session) return;
        const request = pending.get(event.data.requestId);
        if (!request) return;
        pending.delete(event.data.requestId);
        window.clearTimeout(request.timer);
        if (event.data.ok) request.resolve(event.data.value);
        else request.reject(new Error(String(event.data.value || 'MC-Trin request failed')));
    });
    const request = (action, payload = {}) => new Promise((resolve, reject) => {
        if (closed) { reject(new Error('Plugin document closed')); return; }
        if (pending.size >= 20) { reject(new Error('Too many pending plugin requests')); return; }
        const requestId = String(++sequence);
        const timer = window.setTimeout(() => {
            if (!pending.has(requestId)) return;
            pending.delete(requestId);
            reject(new Error('MC-Trin request timed out'));
        }, 30000);
        pending.set(requestId, { resolve, reject, timer });
        notify('mctrin:request', { requestId, action, ...payload });
    });
    Object.defineProperty(window, 'mctrin', { value: Object.freeze({
        instances: Object.freeze({ list: () => request('instances.list') }),
        run: (actionId) => request('actions.run', { actionId }),
    }) });
    notify('mctrin:ready', {});
    const script = JSON.parse(document.querySelector('#mctrin-plugin-script')?.textContent || '""');
    if (typeof script !== 'string' || script.length > 100000) return;
    if (script.trim()) {
        try {
            // This compilation is confined to the user-enabled opaque iframe. No host
            // window, native invoke function, credentials, or filesystem API is supplied.
            const executeInSandbox = new Function(`"use strict";\n${script}\n//# sourceURL=mctrin-local-plugin.js`);
            executeInSandbox();
        } catch (error) {
            notify('mctrin:error', { message: String(error?.message || error).slice(0, 300) });
        }
    }
})();
