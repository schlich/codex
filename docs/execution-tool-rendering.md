# Execution-style MCP tools

The conversation view recognizes `nushell.evaluate` calls with a string `input`.
It displays the original source, an execution state, available timing and exit
information, and at most three screen rows of output. Empty and null successful
results disappear; failures and stderr precede ordinary output. The existing
transcript action opens the complete received arguments and result, including
wire content, structured content, and MCP metadata. No new keyboard mode is added.

## Tool contract

Verified against Nushell 0.116.0's
[server](https://github.com/nushell/nushell/blob/0.116.0/crates/nu-mcp/src/server.rs)
and [evaluator](https://github.com/nushell/nushell/blob/0.116.0/crates/nu-mcp/src/evaluation.rs).

| Field | Interpretation |
| --- | --- |
| Arguments `input` | Original Nushell source; never reconstructed from output |
| Result `structuredContent.cwd` | Directory **after** evaluation; not necessarily the command's starting directory |
| `history_index`, `timestamp` | Raw transcript metadata; not execution timing |
| `output` | Arbitrary structured value or captured text |
| `note` | Server truncation notice referring to `$history`; remain visible |
| Error `msg`, optional `help` | Diagnostic summary; original labels and details stay in the transcript |
| Output `{stdout, stderr, exit_code}` | Conventional `complete` record, recognized only with these three correctly typed fields |

The evaluator returns a NUON text block and a JSON mirror in `structuredContent`.
The adapter interprets that mirror, avoiding a Nushell parser dependency. Legacy
or unrecognized payloads show their received content. A valid JSON mirror is the
authoritative presentation source; differing text remains available in the raw
transcript. Unknown metadata is retained without assigning it execution meaning.

A bare external command combines stdout and stderr and does **not** expose its
exit status. Such a successful MCP evaluation is labeled **Completed**, never
`exit 0`. A correctly typed conventional `complete` record supplies the reported
exit: zero is **Succeeded**, nonzero is **Failed**. It is a shape convention,
not proof about how the source constructed the record. An MCP or transport error
always remains **Failed**. Nushell background promotion is an MCP error with a
diagnostic; the renderer does not pretend to follow the promoted job.

Duration comes from Codex's call lifecycle, not Nushell's timestamp. Absent or
invalid historical durations remain unknown. Nushell currently emits no MCP
progress notifications, so the existing in-flight indicator supplies running
state. No additional progress transport is introduced.

Source uses the existing syntax theme and `nu` grammar lookup, with plain text
fallback if the installed grammar bundle lacks Nushell or highlighting exceeds
its safety limits. It is not highlighted as Bash. Existing source-aware wrapping
preserves logical lines and indentation for copying and selection.

Full transcript means all data **received by Codex**. It cannot recover separate
streams that the server combined, or output the server withheld behind a history
reference. Original wire blocks preserve annotations and escaped line endings.

## Tests and deterministic demonstration

From `codex-rs`, use the existing development shell with an updated overlay (see
the toolchain mismatch below), keeping build artifacts outside the path input:

```nu
$env.CARGO_TARGET_DIR = '/tmp/codex-renderer-target'
$env.RUST_MIN_STACK = '33554432'
def --wrapped codex-test [...args: string] {
    nix develop path:.. --override-input rust-overlay github:oxalica/rust-overlay/a110337465cfb76e3806b81b9db3094c3e6e770a --no-write-lock-file --command cargo test -p codex-tui --lib ...$args
}
codex-test history_cell::mcp
codex-test thread_transcript::tools
codex-test app::transcript_export
codex-test execution_tool_demo -- --ignored --nocapture
```

The demo renders actual history cells, including captured Nushell responses,
through Ratatui's terminal test backend at
80 and 32 columns. It covers running, empty, unknown-exit, successful and failed
process records, evaluation and transport errors, truncation, legacy output,
multiline source, and long Unicode commands. It needs no model endpoint, API key,
or active Nushell server. The same fixtures are snapshot-tested.

The larger test-thread stack accommodates the TUI's app-level MCP fixtures when
running the broader `mcp` filter; it does not change the application renderer.

## Proposed Nix packaging

The checkout already has `packages.x86_64-linux.codex-rs` and a Rust package
expression using `codex-rs/Cargo.lock`. This renderer adds no Rust dependencies,
so it requires no renderer-specific packaging override or new cargo vendor hashes.

The checkout's locked rust-overlay predates its required compiler (it provides
Rust 1.93, while `rust-toolchain.toml` specifies 1.95). For a local candidate,
override that existing input with a newer pinned revision without rewriting the
checkout's lockfile:

```nu
nix build path:/home/schlich/codex#packages.x86_64-linux.codex-rs --override-input rust-overlay github:oxalica/rust-overlay/a110337465cfb76e3806b81b9db3094c3e6e770a --no-write-lock-file --no-link --print-out-paths
```

For reproducible distribution, pin the reviewed patched source revision in a
separate consumer flake, override its rust-overlay input with the pinned updated
overlay, consume `packages.x86_64-linux.codex-rs`, and commit that consumer's
lockfile. To match validation exactly, instantiate the existing package expression
with a Rust platform built from `rust-bin.stable."1.95.0".minimal` in that consumer.
Keep configuration wrappers separate from the package;
the patched binary accepts the same arguments as upstream. The source flake's
development toolchain must meet the checkout's `rust-toolchain.toml`. This is a
packaging proposal; the full CLI package build is separate from the focused TUI
test build.

Adopting the candidate in dotfiles is a separate decision. No Home Manager,
NixOS, Codex wrapper, or active installation changes are required for this patch.
