/**
 * Behavioural probe for the DSH capture bridge's `apply()` wiring.
 *
 * It mounts the real plugin with a fake Cordis context, emits one `turn/end`,
 * and lets a stub engine executable record the argv the bridge spawns. This
 * exercises the listener registration, the hint-then-capture order, the
 * projection publication, the compaction wait, and the `--thread` contract
 * without a running DSH.
 *
 * Usage: node apply_probe.mjs <bridge.mjs> <stub-executable> <state-dir> <log> [mode]
 *
 * Modes: `ready` (default), `compact` (start a bracket before turn/end and close
 * it after the hint), or `deferred` (the engine reports an unregistered project).
 */

import { existsSync, readFileSync } from 'node:fs'
import { pathToFileURL } from 'node:url'

const [bridgePath, stub, stateDir, logPath, mode = 'ready'] = process.argv.slice(2)
if (!bridgePath || !stub || !stateDir || !logPath) {
  console.error('usage: apply_probe.mjs <bridge.mjs> <stub> <state-dir> <log> [mode]')
  process.exit(2)
}

process.env.BRIDGE_PROBE_LOG = logPath
const bridge = await import(pathToFileURL(bridgePath).href)

const topLevel = new Map()
const sessionListeners = []
const agent = {
  session: {
    id: 'session-x',
    header: { isSeeded: false, cwd: '/repo' },
  },
  ctx: {
    effect: (fn) => fn(),
    on: (_name, fn) => {
      sessionListeners.push(fn)
      return () => {}
    },
  },
}
const ctx = {
  on: (name, fn) => {
    if (!topLevel.has(name)) topLevel.set(name, [])
    topLevel.get(name).push(fn)
    return () => {}
  },
  sessionQuery: {
    readSession: async (id) => ({
      session: { id, version: 4, createdAt: 1, cwd: '/repo', isSeeded: false },
      events: [
        { seq: 0, type: 'turn/start', time: 1000, data: { turn: 1 } },
        {
          seq: 1,
          type: 'user/message',
          time: 1001,
          data: {
            id: 'm1',
            role: 'user',
            content: [{ type: 'text', text: 'hello' }],
            source: { kind: 'user' },
          },
        },
        { seq: 2, type: 'turn/end', time: 1002, data: { turn: 1, reason: { kind: 'completed' } } },
      ],
    }),
  },
}

bridge.apply(ctx, { executable: stub, stateDir, dshHome: stateDir, timeoutMs: 10000 })
for (const listener of topLevel.get('agent/created') ?? []) listener({ agent })
if (sessionListeners.length !== 1) {
  console.error(`expected one session listener, got ${sessionListeners.length}`)
  process.exit(1)
}

const emit = (event) => sessionListeners[0](agent.session, event)
const projection = `${stateDir}/dsh/projections/session-x.1.json`
const lines = () => {
  try {
    return readFileSync(logPath, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse)
  } catch {
    return []
  }
}
const waitFor = async (predicate, timeoutMs = 5000) => {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (predicate()) return true
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
  return false
}

if (mode === 'compact') emit({ type: 'compaction/start', data: {} })
emit({ type: 'turn/end', data: { turn: 1, reason: { kind: 'completed' } } })

let result
if (mode === 'deferred') {
  // The project is not registered: the hint is kept and the projection is
  // published for a later register/reconcile, but no capture is attempted.
  const published = await waitFor(() => existsSync(projection))
  result = { mode, lines: lines(), projection: existsSync(projection), published }
} else if (mode === 'compact') {
  // The hint is sent immediately; the capture must wait for the bracket.
  const hintSent = await waitFor(() => lines().length >= 1, 2000)
  const waiting = lines().length === 1
  emit({ type: 'compaction/end', data: {} })
  const complete = await waitFor(() => lines().length >= 2 && !existsSync(projection))
  result = {
    mode,
    lines: lines(),
    projection: existsSync(projection),
    hintSent,
    waitingWhileCompacting: waiting,
    complete,
  }
} else if (mode === 'held') {
  // A held result keeps its projection for an explicit retry.
  const complete = await waitFor(() => lines().length >= 2, 5000)
  result = { mode, lines: lines(), projection: existsSync(projection), complete }
} else {
  const complete = await waitFor(() => lines().length >= 2 && !existsSync(projection))
  result = { mode, lines: lines(), projection: existsSync(projection), complete }
}

if (
  !result.complete
  && !(mode === 'deferred' && result.published)
) {
  console.error(JSON.stringify(result))
  process.exit(1)
}
if (mode === 'compact' && (!result.hintSent || !result.waitingWhileCompacting)) {
  console.error(JSON.stringify(result))
  process.exit(1)
}
console.log(JSON.stringify(result))
process.exitCode = 0
