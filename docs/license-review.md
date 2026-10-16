# Distribution license review

Reviewed on 2026-10-16 (Asia/Tokyo). The owner approved `MIT OR Apache-2.0`
for original project material on the same date. The root license files state
the actual grants; this document records the review and third-party exceptions.

## Recommendation

Use `MIT OR Apache-2.0` for original Coding Memory code and original repository
documentation. Recipients may choose either license for that original work.
Third-party components retain their own licenses and notices; the choice does
not relicense the assembled executable or upstream material under MIT alone.

Both licenses permit commercial use, modification, and redistribution subject
to their conditions. Apache-2.0 includes an express contributor patent grant
with stated limits and patent-litigation termination. MIT offers a short license
with notice retention. Offering both gives recipients a familiar Rust licensing
choice, also used by the [Rust project](https://rust-lang.org/policies/licenses/).
An MIT-only project license would also be a plausible choice for our original
work, but would not remove Apache-2.0 obligations from dependencies.

Sources: [MIT](https://opensource.org/license/mit),
[Apache-2.0 sections 2–4](https://www.apache.org/licenses/LICENSE-2.0).

The project license should cover only material the owner and contributors have
the right to license. The separate Markdown Vault, captured conversations,
evidence, credentials, and user-generated memories are outside this software
license selection and must not be included in release archives.

## Scope and verification

The initial distribution target is `aarch64-apple-darwin`. The inventory below
comes from the committed Cargo.lock and locally downloaded crate manifests:

```sh
cargo metadata --locked --offline --filter-platform aarch64-apple-darwin --format-version 1
cargo tree --locked --offline --target aarch64-apple-darwin --edges normal,build
```

The metadata inventory contains 104 third-party package versions, including
build tools, procedural macros, and packages participating in development
feature resolution. It is a conservative dependency inventory, not proof that
every package's code is present in the final executable. The release workflow
must regenerate its inventory using the actual release features and target.

No GPL, LGPL, AGPL, MPL, or noncommercial-only expression appears in this
target's Cargo metadata. That is a metadata observation, not a whole-binary
license certification. License files were enumerated in the downloaded crates;
special cases below received additional inspection. No exhaustive source
similarity comparison against reference projects was performed.

An unfiltered offline metadata request could not complete because a dependency
for another platform was not downloaded. Other targets remain outside this
review. Rust standard-library/runtime notices, system libraries, and any future
installer payload also need checking against the actual release artifact.

## Findings requiring release packaging work

| Component | Finding | Required handling |
| --- | --- | --- |
| Most Rust crates | MIT and/or Apache-2.0; legacy `MIT/Apache-2.0` expressions also occur | Preserve the selected upstream licenses and required notices; inspect legacy expressions rather than treating `/` as a new SPDX operator |
| `rmcp` and `rmcp-macros` 3.5.1 | Cargo declares Apache-2.0, but published crates contain no LICENSE file; the source repository explains a transition with some contributions remaining MIT | Include the upstream transition statement, Apache-2.0 text, and retained MIT copyright/permission notice; do not infer a blanket MIT option for all SDK code |
| `unicode-ident` 1.0.27 | `(MIT OR Apache-2.0) AND Unicode-3.0` | Retain the Unicode copyright and permission notice in addition to the chosen code license |
| `foldhash` 0.2.0 | Zlib | Preserve its notice, respect origin and modified-source requirements |
| `ryu` 1.0.23 | Apache-2.0 OR BSL-1.0 | Choose and include an allowed license; BSL-1.0 here means Boost Software License, not Business Source License |
| `rustix` 1.1.5 | MIT, Apache-2.0, or Apache-2.0 WITH LLVM-exception alternatives | Preserve the applicable license and attribution; inspect bundled COPYRIGHT and license files |
| `tracing-core` 0.1.36 | An additional MIT license exists under `src/spin/LICENSE` | Include the nested attribution when packaging notices conservatively |
| `rusqlite` / `libsqlite3-sys` | Rust wrapper is MIT; `bundled` compiles SQLite into the executable | Preserve wrapper notices and record SQLite's public-domain dedication separately |
| `libsqlite3-sys/sqlcipher` | Downloaded crate also contains SQLCipher material; current configuration uses bundled SQLite, not SQLCipher | Do not mistake all files in a crate archive for linked components; reassess if SQLCipher features are enabled |

The SDK source revision is
[`79437f291b2c44053d00dcd5db969fd0cca7c887`](https://github.com/modelcontextprotocol/rust-sdk/blob/79437f291b2c44053d00dcd5db969fd0cca7c887/LICENSE),
confirmed from both crates' `.cargo_vcs_info.json`. Its root NOTICE lookup
returned 404 at that revision. Its LICENSE also assigns non-specification
documentation to CC-BY-4.0; copying such documentation would require a separate
attribution review. Do not apply that documentation license indiscriminately to
the compiled SDK code.

Additional sources: [SQLite dedication](https://sqlite.org/copyright.html),
[Unicode-3.0](https://spdx.org/licenses/Unicode-3.0.html),
[Zlib](https://spdx.org/licenses/Zlib.html).

## Reference projects and external runtimes

The existing [runtime research](runtime-research.md#licensing),
[capture experiment](codex-capture-experiment.md), and
[generation experiment](codex-generation-experiment.md) state that upstream
implementation code was not copied. Searches of source, tests, experiments,
and documentation found no additional copied-code declaration. This supports
treating these as design/protocol references; it does not independently prove
the origin of every line.

Upstream license files were fetched again at the recorded revisions:

| Reference | Revision | Verified license / notice | Distribution consequence |
| --- | --- | --- | --- |
| [agent-memory](https://github.com/tigerless-labs/agent-memory/blob/3698e601e07665121b9388aca100364837518e20/LICENSE) | `3698e601` | MIT; Tigerless Labs attribution | Retain the research citation; copy the required notices if implementation is incorporated later |
| [Anamnesis](https://github.com/Trapezohe/Anamnesis/blob/9d894360f96ea05ff30205c08c8d07ba9725f7f9/LICENSE) | `9d894360` | Apache-2.0 and [NOTICE](https://github.com/Trapezohe/Anamnesis/blob/9d894360f96ea05ff30205c08c8d07ba9725f7f9/NOTICE) | Actual code reuse would retain applicable notices and identify modified files; its code could not simply be offered under our MIT alternative |
| [DeepSeek Harness](https://github.com/shiguredo/deepseek-harness/blob/975e4aff0c6a3c90af42005eef557b8720dd0a00/LICENSE) | `975e4aff` | MIT; DeepSeek attribution | Retain the research citation; check per-file and bundled dependency notices for any future copying |
| [Codex CLI](https://github.com/openai/codex/blob/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/LICENSE) | `092d3acd` | Apache-2.0 and [NOTICE](https://github.com/openai/codex/blob/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/NOTICE), including Ratatui attribution | Keep Codex separately installed; bundling its binary or copying code requires its own distribution inventory |

Protocol interoperability and separately launching an installed runtime do not
by themselves establish that Coding Memory contains that runtime's code. The
ChatGPT/Codex desktop application is not part of the proposed release payload;
this review does not authorize redistributing it or its bundled runtime.

Nix, pkfire, Cargo, cargo-dist, and GitHub Actions are development/release tools.
Their licenses do not automatically become the project's license. Generated
scripts, templates, or any tool binaries actually included in an installer must
be reviewed as shipped material. The future cargo-dist version and generated
files have not yet been selected or audited here.

## Release packaging requirements

1. Add LICENSE-MIT and LICENSE-APACHE for original project material, set Cargo's
   `license = "MIT OR Apache-2.0"`, and explain the third-party exceptions in
   README. Keep `publish = false` until crates.io distribution is intended.
2. Generate versioned third-party notices from the locked release dependency
   graph, including nested notices and a pinned SDK license supplement. Include
   them in every archive and preserve them through Homebrew installation.
3. Make release checks reject missing license text, missing applicable notices,
   or unreviewed license expressions. Metadata-only scanners are insufficient
   for the SDK case discovered above.
4. Review the actual archive, linked runtime/system dependencies, and generated
   installer material. Record source versions and checksums so this review can
   be refreshed when dependencies or targets change.
5. Keep design-reference acknowledgments separate from notices for incorporated
   code, so attribution does not imply code was copied or rights were granted.

## Locked dependency inventory

These are upstream manifest declarations, not replacement license grants.
This is the pre-release baseline inventory; the lock hash below predates the
root package version change to `0.1.0-rc.1`. Release archives carry a regenerated
inventory and lock hash for their own build.

Cargo.lock SHA-256: `8285bc766aa711f273a3db2fd1b023d64812e8280e690fd4fb7e7fbe0cd96ede`.

| Package | Version | Declared license |
| --- | --- | --- |
| `anstream` | 1.0.0 | `MIT OR Apache-2.0` |
| `anstyle` | 1.0.14 | `MIT OR Apache-2.0` |
| `anstyle-parse` | 1.0.0 | `MIT OR Apache-2.0` |
| `anstyle-query` | 1.1.5 | `MIT OR Apache-2.0` |
| `anyhow` | 1.0.104 | `MIT OR Apache-2.0` |
| `autocfg` | 1.5.1 | `Apache-2.0 OR MIT` |
| `bitflags` | 2.13.2 | `MIT OR Apache-2.0` |
| `block-buffer` | 0.12.1 | `MIT OR Apache-2.0` |
| `bytes` | 1.12.1 | `MIT` |
| `cc` | 1.8.0 | `MIT OR Apache-2.0` |
| `cfg-if` | 1.0.5 | `MIT OR Apache-2.0` |
| `chrono` | 0.4.45 | `MIT OR Apache-2.0` |
| `clap` | 4.6.7 | `MIT OR Apache-2.0` |
| `clap_builder` | 4.6.7 | `MIT OR Apache-2.0` |
| `clap_derive` | 4.6.7 | `MIT OR Apache-2.0` |
| `clap_lex` | 1.1.1 | `MIT OR Apache-2.0` |
| `colorchoice` | 1.0.5 | `MIT OR Apache-2.0` |
| `const-oid` | 0.10.2 | `Apache-2.0 OR MIT` |
| `core-foundation-sys` | 0.8.7 | `MIT OR Apache-2.0` |
| `cpufeatures` | 0.3.1 | `MIT OR Apache-2.0` |
| `crypto-common` | 0.2.2 | `MIT OR Apache-2.0` |
| `darling` | 0.24.1 | `MIT` |
| `darling_core` | 0.24.1 | `MIT` |
| `darling_macro` | 0.24.1 | `MIT` |
| `digest` | 0.11.3 | `MIT OR Apache-2.0` |
| `dyn-clone` | 1.0.20 | `MIT OR Apache-2.0` |
| `equivalent` | 1.0.3 | `Apache-2.0 OR MIT` |
| `errno` | 0.3.14 | `MIT OR Apache-2.0` |
| `fallible-iterator` | 0.3.0 | `MIT/Apache-2.0` |
| `fallible-streaming-iterator` | 0.1.9 | `MIT/Apache-2.0` |
| `fastrand` | 2.5.0 | `Apache-2.0 OR MIT` |
| `find-msvc-tools` | 0.1.14 | `MIT OR Apache-2.0` |
| `foldhash` | 0.2.0 | `Zlib` |
| `futures` | 0.3.34 | `MIT OR Apache-2.0` |
| `futures-channel` | 0.3.34 | `MIT OR Apache-2.0` |
| `futures-core` | 0.3.34 | `MIT OR Apache-2.0` |
| `futures-executor` | 0.3.34 | `MIT OR Apache-2.0` |
| `futures-io` | 0.3.34 | `MIT OR Apache-2.0` |
| `futures-macro` | 0.3.34 | `MIT OR Apache-2.0` |
| `futures-sink` | 0.3.34 | `MIT OR Apache-2.0` |
| `futures-task` | 0.3.34 | `MIT OR Apache-2.0` |
| `futures-util` | 0.3.34 | `MIT OR Apache-2.0` |
| `getrandom` | 0.4.3 | `MIT OR Apache-2.0` |
| `hashbrown` | 0.17.1 | `MIT OR Apache-2.0` |
| `hashlink` | 0.12.2 | `MIT OR Apache-2.0` |
| `heck` | 0.5.0 | `MIT OR Apache-2.0` |
| `hybrid-array` | 0.4.15 | `MIT OR Apache-2.0` |
| `iana-time-zone` | 0.1.65 | `MIT OR Apache-2.0` |
| `ident_case` | 1.0.1 | `MIT/Apache-2.0` |
| `indexmap` | 2.14.2 | `Apache-2.0 OR MIT` |
| `is_terminal_polyfill` | 1.70.2 | `MIT OR Apache-2.0` |
| `itoa` | 1.0.18 | `MIT OR Apache-2.0` |
| `libc` | 0.2.190 | `MIT OR Apache-2.0` |
| `libsqlite3-sys` | 0.38.2 | `MIT` |
| `memchr` | 2.8.3 | `Unlicense OR MIT` |
| `mio` | 1.2.4 | `MIT` |
| `num-traits` | 0.2.19 | `MIT OR Apache-2.0` |
| `once_cell` | 1.21.4 | `MIT OR Apache-2.0` |
| `pastey` | 0.2.3 | `MIT OR Apache-2.0` |
| `pin-project-lite` | 0.2.17 | `Apache-2.0 OR MIT` |
| `pkg-config` | 0.3.34 | `MIT OR Apache-2.0` |
| `proc-macro2` | 1.0.107 | `MIT OR Apache-2.0` |
| `quote` | 1.0.47 | `MIT OR Apache-2.0` |
| `ref-cast` | 1.0.27 | `MIT OR Apache-2.0` |
| `ref-cast-impl` | 1.0.27 | `MIT OR Apache-2.0` |
| `rmcp` | 3.5.1 | `Apache-2.0` |
| `rmcp-macros` | 3.5.1 | `Apache-2.0` |
| `rusqlite` | 0.40.2 | `MIT` |
| `rustix` | 1.1.5 | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` |
| `ryu` | 1.0.23 | `Apache-2.0 OR BSL-1.0` |
| `schemars` | 1.2.3 | `MIT` |
| `schemars_derive` | 1.2.3 | `MIT` |
| `semver` | 1.0.28 | `MIT OR Apache-2.0` |
| `serde` | 1.0.229 | `MIT OR Apache-2.0` |
| `serde_core` | 1.0.229 | `MIT OR Apache-2.0` |
| `serde_derive` | 1.0.229 | `MIT OR Apache-2.0` |
| `serde_derive_internals` | 0.30.0 | `MIT OR Apache-2.0` |
| `serde_json` | 1.0.154 | `MIT OR Apache-2.0` |
| `serde_yaml_ng` | 0.10.0 | `MIT` |
| `sha2` | 0.11.1 | `MIT OR Apache-2.0` |
| `shlex` | 2.0.1 | `MIT OR Apache-2.0` |
| `signal-hook-registry` | 1.4.8 | `MIT OR Apache-2.0` |
| `slab` | 0.4.12 | `MIT` |
| `smallvec` | 1.16.3 | `MIT OR Apache-2.0` |
| `socket2` | 0.6.5 | `MIT OR Apache-2.0` |
| `strsim` | 0.11.1 | `MIT` |
| `syn` | 2.0.119 | `MIT OR Apache-2.0` |
| `syn` | 3.0.9 | `MIT OR Apache-2.0` |
| `tempfile` | 3.27.0 | `MIT OR Apache-2.0` |
| `thiserror` | 2.0.21 | `MIT OR Apache-2.0` |
| `thiserror-impl` | 2.0.21 | `MIT OR Apache-2.0` |
| `tokio` | 1.53.3 | `MIT` |
| `tokio-macros` | 2.7.2 | `MIT` |
| `tokio-util` | 0.7.20 | `MIT` |
| `tracing` | 0.1.44 | `MIT` |
| `tracing-attributes` | 0.1.31 | `MIT` |
| `tracing-core` | 0.1.36 | `MIT` |
| `typenum` | 1.20.1 | `MIT OR Apache-2.0` |
| `unicode-ident` | 1.0.27 | `(MIT OR Apache-2.0) AND Unicode-3.0` |
| `unsafe-libyaml` | 0.2.11 | `MIT` |
| `utf8parse` | 0.2.2 | `Apache-2.0 OR MIT` |
| `uuid` | 1.28.0 | `Apache-2.0 OR MIT` |
| `vcpkg` | 0.2.15 | `MIT/Apache-2.0` |
| `zmij` | 1.0.23 | `MIT` |
