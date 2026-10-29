//! Per-authority HTTP/2 versus HTTP/1 preference, learned from failures.

use super::*;

pub(crate) static H2_FALLBACK_AUTHORITIES: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub(crate) static HTTP1_PREFERENCE_AUTHORITIES: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
pub(crate) fn authority_uses_http1_fallback(authority: &str) -> bool {
    authority_uses_http1_fallback_for(authority, ProxyPolicy::System)
}

pub(crate) fn authority_uses_http1_fallback_for(
    authority: &str,
    proxy: ProxyPolicy,
) -> bool {
    let authority =
        super::super::download::proxy_context::authority_key(authority, proxy);
    {
        let mut preferences = HTTP1_PREFERENCE_AUTHORITIES.lock();
        preferences.retain(|_, until| *until > Instant::now());
        if preferences.contains_key(&authority) {
            return true;
        }
    }
    let mut fallbacks = H2_FALLBACK_AUTHORITIES.lock();
    match fallbacks.get(&authority) {
        Some(until) if *until > Instant::now() => true,
        Some(_) => {
            fallbacks.remove(&authority);
            false
        }
        None => false,
    }
}

#[cfg(test)]
pub(crate) fn record_authority_h2_failure(authority: &str) {
    record_authority_h2_failure_for(authority, ProxyPolicy::System);
}

pub(crate) fn record_authority_h2_failure_for(
    authority: &str,
    proxy: ProxyPolicy,
) {
    let now = Instant::now();
    let mut fallbacks = H2_FALLBACK_AUTHORITIES.lock();
    fallbacks.retain(|_, until| *until > now);
    fallbacks.insert(
        super::super::download::proxy_context::authority_key(authority, proxy),
        now + H2_FALLBACK_TTL,
    );
}

pub(crate) fn prefer_measured_http1(authority: &str, proxy: ProxyPolicy) {
    let mut preferences = HTTP1_PREFERENCE_AUTHORITIES.lock();
    preferences.retain(|_, until| *until > Instant::now());
    preferences.insert(
        super::super::download::proxy_context::authority_key(authority, proxy),
        Instant::now() + H2_SLOW_FALLBACK_TTL,
    );
}

pub(crate) fn is_h2_protocol_failure(error: &reqwest::Error) -> bool {
    let mut chain = String::new();
    let mut source = error.source();
    while let Some(next) = source {
        if !chain.is_empty() {
            chain.push(' ');
        }
        chain.push_str(&next.to_string());
        source = next.source();
    }
    let chain = chain.to_ascii_lowercase();
    [
        FETCH_HTTP2,
        FETCH_HTTP_2,
        FETCH_GOAWAY,
        FETCH_STREAM_ERROR,
        FETCH_PROTOCOL_ERROR,
        FETCH_REFUSED_STREAM,
    ]
    .iter()
    .any(|marker| chain.contains(marker))
}
