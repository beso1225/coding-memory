# Codex Capture Experiment

This is an opt-in runtime probe for [issue #1](https://github.com/beso1225/coding-memory/issues/1),
not the production adapter. See [the experiment report](../../docs/codex-capture-experiment.md).

The scripts use only Python's standard library. A fresh directory under
`/private/tmp/coding-memory-capture-*` contains the synthetic project, isolated
`CODEX_HOME`, hook logs, and result. A loopback HTTP server supplies deterministic
Responses SSE events. No personal transcript or authentication file is copied.
The installed runtime, not a mocked Codex implementation, executes the hooks,
tools, turn lifecycle, history persistence, and history APIs.

Run from the repository root on macOS:

```sh
nix develop --command python3 experiments/codex_capture/run.py /opt/homebrew/bin/codex
nix develop --command python3 experiments/codex_capture/run.py \
  /Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex
```

The program prints its result directory and a case summary. It tests exec
completion/failure/SIGINT, Stop continuation, a harmless `printf` tool call,
steering, API failure/interruption, later-turn isolation, pagination, cold reads,
fork inheritance, resume, and explicit compaction. Response/item IDs differ
between requests. The failure is a synthetic HTTP 400, not a tool exit failure.

The probe enables hook trust bypass only for the hook scripts it creates in its
isolated home. This establishes lifecycle behavior with trusted hooks; it does
not test GUI trust approval or install collection hooks in the user's setup.
The model provider needs no credentials. Plugins are disabled in the probe's
configuration. These settings are not a general network-isolation guarantee.
Earlier exploratory runs with plugins enabled attempted an unauthenticated
featured-plugin request; production generation isolation belongs to issue #2.

Raw `result.json` files contain synthetic runtime history and generated runtime
instructions. Keep them outside Git. Export a smaller projection for review:

```sh
python3 experiments/codex_capture/export_fixture.py \
  /private/tmp/coding-memory-capture-EXAMPLE/result.json \
  experiments/codex_capture/fixtures/cli-0.162.1.json
pkf run capture-fixtures
```

Use the corresponding bundled-version filename for the second probe. The export
removes local paths and runtime instruction bodies. The committed fixtures keep
observed field names and native IDs, while omitting some metadata. They are API
projections, not byte-complete rollout files or raw-format decoder fixtures.
Their inherited byte offsets refer to the original synthetic rollout and must
not be used as offsets in the exported JSON.

The offline assertions run in `pkf run check` and CI. They never launch Codex,
read a runtime state directory, or call a provider. Re-run the opt-in probe when
changing supported Codex versions rather than treating these fixture checks as
proof of behavior in a newly installed runtime.

## Remaining ChatGPT App GUI check

The bundled executable can be tested above without operating ChatGPT App.
The actual App-owned process has also been checked for its executable and open
state-file locations, but App GUI hook execution remains unverified. The available
Computer Use tool rejected control of this App (`com.openai.codex`), so the GUI
step needs a human in this environment.

Prepare a separate test project without changing regular Codex settings:

```sh
nix develop --command python3 experiments/codex_capture/prepare_app_probe.py
```

Open the printed `project` path as a local project in ChatGPT App. The generated
project-local configuration uses absolute executable paths and logs only metadata
for that project's exact working directory. It does not record conversation text.
The printed `events` path identifies the log to inspect after the test.

Before closing issue #1, use a separate synthetic project in ChatGPT App:

1. Review the prepared project's hook script and project-local `.codex` configuration.
2. Review and trust those hooks through the App's supported flow. Do not transfer
   the probe's trust bypass to the user's regular configuration.
3. Start a local App turn asking for a literal synthetic answer, then perform
   another turn with a harmless tool call and interrupt a third turn.
4. Record the hook/notification payloads, trust behavior, and executable environment.
   Use a separately started matching bundled app-server to cold-read only those
   known synthetic thread IDs without resuming them.
5. Repeat after an App restart. Record any inability to enable hooks, access the
   state store, or read the selected histories as a design checkpoint.

Do not infer success here from the isolated bundled-binary probe. No App GUI
conversation was created or sent by this experiment.
