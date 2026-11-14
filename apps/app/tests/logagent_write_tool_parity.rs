//! Parity between the Rust write-approval gate and the renderer's copies of
//! the write list.
//!
//! `assistant_write_tool` decides whether the backend mints a grant, while the
//! renderer decides whether it asks the user and attaches that grant. A name
//! that exists only in Rust is refused after the model already tried to act; a
//! name that exists only in the renderer is sent without a grant and refused
//! by the gate. Neither failure shows up in a type check, so the lists are
//! compared here, following the textual-parity pattern of
//! `instance_command_manifest_parity.rs`.

use std::collections::BTreeSet;

/// Reads a list body and keeps every identifier in it, ignoring quotes,
/// commas, `|` alternatives and layout. Both sides declare plain identifiers,
/// so this works for the Rust slice and the TypeScript `Set`.
fn identifiers_in(
    source: &str,
    marker: &str,
    terminator: &str,
) -> BTreeSet<String> {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing list marker: {marker}"))
        + marker.len();
    let end = source[start..]
        .find(terminator)
        .unwrap_or_else(|| panic!("unterminated list after marker: {marker}"))
        + start;
    source[start..end]
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '_')
        })
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn rust_write_tools() -> BTreeSet<String> {
    identifiers_in(
        include_str!("../../../packages/app-lib/src/api/logs/logagent/mod.rs"),
        "pub(crate) const ASSISTANT_WRITE_TOOLS: &[&str] = &[",
        "];",
    )
}

fn renderer_write_tools() -> BTreeSet<String> {
    identifiers_in(
        include_str!(
            "../../../apps/app-frontend/src/logagent/tool-settings.ts"
        ),
        "export const ASSISTANT_WRITE_TOOLS: ReadonlySet<string> = new Set([",
        "])",
    )
}

#[test]
fn the_renderer_write_list_matches_the_rust_approval_gate() {
    let rust = rust_write_tools();
    let renderer = renderer_write_tools();

    assert_eq!(
        rust.len(),
        11,
        "the write list changed size; update every mirror and this count: {rust:?}"
    );
    assert_eq!(
        rust, renderer,
        "the Rust approval gate and the renderer write list must name the same \
         tools: a Rust-only entry is refused after the model acts, and a \
         renderer-only entry is sent without a grant"
    );
}

/// The gate only protects names `assistant_tool_name` allows. A write tool that
/// is missing there could never be requested, and a name added only to
/// `assistant_tool_name` would run without a grant, so the write list has to be
/// a subset of the assistant tool list.
#[test]
fn every_write_tool_is_an_assistant_tool() {
    let module =
        include_str!("../../../packages/app-lib/src/api/logs/logagent/mod.rs");
    // The terminator is the closing brace of the function itself, not the next
    // `fn`: the write list constant sits in between, and swallowing it would
    // make this subset assertion pass by construction.
    let assistant = identifiers_in(
        module,
        "fn assistant_tool_name(name: &str) -> bool {",
        "\n}\n",
    );
    assert!(
        assistant.len() > rust_write_tools().len(),
        "assistant_tool_name was not parsed; the subset check below would be empty"
    );
    let missing = rust_write_tools()
        .difference(&assistant)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "write tools missing from assistant_tool_name: {}",
        missing.join(", ")
    );
}

/// A write tool the model can see but `agent.ts` never registers cannot be
/// called at all, which makes the approval gate look broken rather than unused.
#[test]
fn every_write_tool_is_registered_by_the_agent() {
    let agent =
        include_str!("../../../apps/app-frontend/src/logagent/agent.ts");
    for tool in renderer_write_tools() {
        assert!(
            agent.contains(&format!("name: '{tool}'")),
            "{tool} is a write tool but agent.ts never registers it"
        );
    }
}
