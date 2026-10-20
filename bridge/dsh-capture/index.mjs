/**
 * @coding-memory/dsh-capture — the DeepSeek Harness side of Coding Memory capture.
 *
 * The plugin never decides what becomes memory. On a completed turn it writes a
 * bounded completion hint, and when a session ends it writes a bounded
 * projection of that session read through DSH's own `sessionQuery`. The Rust
 * engine validates, scopes, and stores everything.
 *
 * Mount it as an out-of-tree Cordis plugin with a `--patch` overlay:
 *
 *   - insert:
 *       - id: coding-memory-capture
 *         name: '/absolute/bridge/dsh-capture/index.mjs'
 *         config:
 *           executable: /absolute/coding-memory
 *           stateDir: /absolute/engine-state
 *           dshHome: /absolute/dsh-home
 *
 * @module @coding-memory/dsh-capture
 */

import { spawn } from 'node:child_process'
import { mkdir, rename, rm, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { dirname, isAbsolute, join } from 'node:path'

/** Cordis plugin name. */
export const name = 'coding-memory-capture'
/** Services required to observe turns and read session history. */
export const inject = ['sessions', 'sessionQuery']

/**
 * The plugin's configuration schema in Cordis's standard-schema shape, keeping
 * this bridge dependency-free. `apply` validates the three absolute paths.
 */
export const Config = {
  '~standard': {
    version: 1,
    vendor: 'coding-memory',
    validate(value) {
      if (typeof value !== 'object' || value === null || Array.isArray(value)) {
        return { issues: [{ message: 'config must be an object' }] }
      }
      const issues = []
      for (const field of ['executable', 'stateDir', 'dshHome']) {
        const given = value[field]
        if (typeof given !== 'string' || given.length === 0) {
          issues.push({ message: `${field} must be a non-empty string` })
        } else if (!isAbsolute(given)) {
          // A relative path resolves against the harness process's cwd, which
          // is the user's project, not the installation.
          issues.push({ message: `${field} must be an absolute path` })
        } else if (/[\u0000-\u001f]/.test(given)) {
          issues.push({ message: `${field} must not contain control characters` })
        }
      }
      if (value.timeoutMs !== undefined
        && !(typeof value.timeoutMs === 'number' && Number.isFinite(value.timeoutMs) && value.timeoutMs > 0)) {
        issues.push({ message: 'timeoutMs must be a positive number' })
      }
      return issues.length > 0 ? { issues } : { value }
    },
  },
}

/** How many times one capture may retry while the engine reports pending. */
export const CAPTURE_ATTEMPTS = 12
/**
 * Delay between capture retries, in milliseconds.
 *
 * It must not be shorter than the engine's ten-second deferral after
 * `history_pending`: an earlier retry finds the run unavailable, reports
 * `no_ready_run`, and used to discard a projection whose run was still queued.
 */
export const CAPTURE_RETRY_MS = 10000
/** How long capture waits for a compaction request announced at turn end. */
export const COMPACTION_SETTLE_MS = 1500
/** Markers a producer emits when it is about to compact one session. */
export const COMPACTION_REQUEST_EVENTS = ['coding-memory/compaction-requested']
/** Largest projection the bridge will write, in bytes. */
export const MAX_PROJECTION_BYTES = 16 * 1024 * 1024
/** Largest completion hint the bridge will write, in bytes. */
export const MAX_SIGNAL_BYTES = 65536
/** Session ids containing this marker are Coding Memory's own generation runs. */
export const ENGINE_SESSION = 'coding-memory-'

/**
 * The engine's capture report, or an empty object when it cannot be parsed.
 * @param output - the engine's stdout.
 * @returns the parsed report.
 */
export function parseReport(output) {
  try {
    const report = JSON.parse(output.trim())
    return report !== null && typeof report === 'object' ? report : {}
  } catch {
    return {}
  }
}

/**
 * Whether a capture report asks for another attempt later.
 *
 * `history_pending` means the log has not recorded the turn's end yet;
 * `no_ready_run` means no run was claimable at that instant, which includes the
 * engine's own deferral window and another capture owning the run. Neither is
 * terminal, so the projection must be kept for the retry.
 * @param reason - the report's reason code, if any.
 * @returns true when another attempt can make progress.
 */
export function retryable(reason) {
  return reason === 'history_pending' || reason === 'no_ready_run'
}

/**
 * Remove a projection whose capture finished.
 *
 * Each capture writes a fresh immutable projection, and before the bounded
 * fallback activates it holds the whole growing session. Keeping those files
 * would grow quadratically and retain raw system and injected records after the
 * filtered evidence is published, so a terminal result deletes its file. A
 * pending capture keeps it for the retry or a later `reconcile`.
 * @param path - projection file.
 */
export async function removeProjection(path) {
  try {
    await rm(path, { force: true })
  } catch {
    // Removal is housekeeping: a leftover file never changes capture results.
  }
}

/**
 * Keep a projected record replayable.
 *
 * A projected record must stay the shape DSH stores: an `assistant/message`
 * whose model source lost its provider fails replay with "message must have
 * model source", and a user message flattened to `{source, turn, step}` fails
 * with "not losslessly JSON-serializable". Projecting only the envelope fields
 * (a fresh object, selected keys) is what keeps replay working; never rewrite a
 * message payload.
 * @param record - one raw log event.
 * @returns the record with its envelope fields, unchanged `data`.
 */
export function projectRecord(record) {
  return {
    seq: record.seq,
    type: record.type,
    time: record.time,
    ...record.surfaceOp === undefined ? {} : { surfaceOp: record.surfaceOp },
    ...record.sourceEventSeqs === undefined ? {} : { sourceEventSeqs: record.sourceEventSeqs },
    ...record.ignorable === undefined ? {} : { ignorable: record.ignorable },
    ...record.data === undefined ? {} : { data: record.data },
  }
}

/**
 * Project one capture, bounded to the turn it names.
 *
 * A long-lived session's complete log eventually exceeds the projection bound,
 * and compaction keeps every raw event, so the whole-session projection would
 * permanently disable capture for that session. The engine only reads one
 * turn's window, so the projection keeps the session header and exactly that
 * turn's records: one contiguous range. Earlier turns, earlier injected
 * context, and later brackets are never read for this capture, and keeping
 * them with their native `seq` values would leave gaps that the engine rejects
 * as `invalid_session_sequence`.
 * @param header - replay-validated session header.
 * @param events - the complete raw log.
 * @param turn - the turn this capture names.
 * @returns the projection text, within {@link MAX_PROJECTION_BYTES}.
 */
export function projectForCapture(header, events, turn) {
  const whole = project(header, events)
  if (Buffer.byteLength(JSON.stringify(whole)) <= MAX_PROJECTION_BYTES) return JSON.stringify(whole)
  const owned = (event) => event.type === 'turn/start' && event.data?.turn === turn
  const closes = (event) => event.type === 'turn/end' && event.data?.turn === turn
  const start = events.findIndex(owned)
  if (start < 0) return JSON.stringify(whole)
  const end = events.findIndex((event, index) => index > start && closes(event))
  const stop = end < 0 ? events.length : end + 1
  // One contiguous range, so every projected record still follows its
  // predecessor. The engine reads nothing outside the turn window, and the
  // bridge must not renumber native `seq` values to patch gaps.
  const bounded = project(header, events.slice(start, stop))
  const text = JSON.stringify(bounded)
  if (Buffer.byteLength(text) > MAX_PROJECTION_BYTES) {
    throw new Error('turn projection exceeds the bridge bound')
  }
  return text
}

/**
 * Refuse a projection DSH could not replay itself.
 *
 * The engine only ever sees the projection, so it cannot validate replay
 * fidelity. Checking it here keeps a projection change from silently producing
 * an unreadable session: the harness reloads these records on resume.
 * @param header - session header.
 * @param records - projected records.
 * @returns the unchanged pair, or throws with the replay failure.
 */
export function assertReplayable(Session, SessionId, header, records) {
  Session.create(SessionId(header.id), records, header, 0, [])
}

/**
 * Project one session read into the document the engine validates.
 * @param header - replay-validated session header from `sessionQuery`.
 * @param events - the complete raw log.
 * @returns a plain JSON document: header fields and contiguous records.
 */
export function project(header, events) {
  return {
    schema: 1,
    header: {
      type: 'session',
      version: header.version,
      id: header.id,
      createdAt: header.createdAt,
      ...header.cwd === undefined ? {} : { cwd: header.cwd },
      isSeeded: header.isSeeded,
      ...header.parentSession === undefined ? {} : { parentSession: header.parentSession },
      ...header.origin === undefined ? {} : { origin: header.origin },
      ...header.delegationDepth === undefined ? {} : { delegationDepth: header.delegationDepth },
    },
    records: events.map(projectRecord),
  }
}

/**
 * Whether a session is Coding Memory's own, which must never be captured.
 * @param sessionId - session identity.
 * @returns true for engine-owned generation sessions.
 */
export function engineOwned(sessionId) {
  return String(sessionId).includes(ENGINE_SESSION)
}

/**
 * Replace `path` atomically so a reader never sees a partial projection.
 * @param path - destination file.
 * @param text - complete document.
 */
export async function publish(path, text) {
  await mkdir(dirname(path), { recursive: true, mode: 0o700 })
  // Two overlapping captures of one session must not share a temporary file:
  // the second rename could publish the first caller's bytes or fail with
  // ENOENT. A per-call suffix keeps every publication self-contained.
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`
  await writeFile(temporary, text, { mode: 0o600 })
  await rename(temporary, path)
}

/**
 * Read a session and publish one turn's projection.
 *
 * The destination is unique per turn, so an overlapping handler for an older
 * turn cannot replace the file a newer engine process is about to read. The
 * same publication serves a deferred turn (project not registered yet): the
 * snapshot is then what `register` and `reconcile` consume, and no capture
 * invocation exists to publish it later.
 * @param stateDir - engine state directory.
 * @param sessionId - session whose projection to write.
 * @param turn - the turn the projection is bounded to.
 * @param read - a function returning a session read.
 * @returns the projection path and the recorded cwd, or null when the session
 *   has no cwd.
 */
export async function publishTurn(stateDir, sessionId, turn, read) {
  const { session, events } = await Promise.resolve().then(read)
  if (session.cwd === undefined) return null
  const path = join(stateDir, 'dsh', 'projections', `${sessionId}.${turn}.json`)
  await publish(path, projectForCapture(session, events, turn))
  return { path, cwd: session.cwd }
}

/**
 * Mount the bridge.
 * @param ctx - Cordis context with `sessionQuery` injected.
 * @param config - validated absolute paths and a bounded timeout.
 */
export function apply(ctx, config) {
  const { executable, stateDir, dshHome, timeoutMs = 15000 } = config
  /**
   * A compaction in flight for one session, from its durable bracket. Capture
   * waits for it so the projection reflects the finished surface: reading a
   * mid-`compaction/start` log would record an unfinished bracket, and a turn
   * that ended just before `/compact` would otherwise be captured before the
   * compaction it is about to be part of.
   * @type {Map<string, {done: Promise<void>, finish: () => void}>}
   */
  const compacting = new Map()
  const settleCompaction = (sessionId) => {
    const id = String(sessionId)
    const entry = compacting.get(id)
    if (entry === undefined) return Promise.resolve()
    return new Promise((resolve) => {
      let settled = false
      const finish = () => {
        if (settled) return
        settled = true
        // Clear the losing timer: an armed Node timer keeps the event loop
        // alive and would otherwise delay teardown after the compaction ended.
        clearTimeout(timer)
        resolve()
      }
      // A bracket that never closes (cancellation, a failing plugin) must not
      // block capture forever, so expiry resolves and retires the stale entry.
      const timer = setTimeout(() => {
        if (compacting.get(id) === entry) compacting.delete(id)
        warn(`compaction of ${id} did not finish within ${timeoutMs} ms; capturing anyway`)
        finish()
      }, timeoutMs)
      entry.done.then(finish)
    })
  }
  /**
   * Wait for a compaction of this session: one already running, or one a
   * producer announced synchronously at the turn's end (`agent.ctx.emit`).
   * The settle window is short and bounded; its result is always a full turn.
   * @param sessionId - session whose ended turn is being captured.
   */
  async function waitForCompaction(sessionId) {
    // Costless when no compaction is announced: a timer here would keep the
    // process alive past the point where the harness tears its services down,
    // which is exactly when this read must already be in flight.
    if (!compacting.has(String(sessionId))) return
    await settleCompaction(sessionId)
  }
  // Diagnostics must reach the harness's own stderr even under `--json`, so
  // they never go through a logger object whose shape varies by composition.
  const warn = (message) => process.stderr.write(`coding-memory-capture: ${message}\n`)

  /** Run one engine subcommand with a bounded wait; diagnostics never escape. */
  async function run(args, input) {
    return await new Promise((resolve) => {
      let output = ''
      let settled = false
      const finish = (result) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        resolve(result)
      }
      const child = spawn(executable, args, { stdio: ['pipe', 'pipe', 'pipe'] })
      const timer = setTimeout(() => {
        child.kill('SIGTERM')
        finish({ code: -1, output: 'engine timed out' })
      }, timeoutMs)
      child.stdout.on('data', (chunk) => {
        if (output.length < 65536) output += String(chunk)
      })
      child.stderr.on('data', (chunk) => warn(`engine: ${String(chunk).trim().slice(0, 500)}`))
      child.on('error', (error) => finish({ code: -1, output: String(error) }))
      child.on('close', (code) => finish({ code: code ?? -1, output }))
      child.stdin.on('error', () => {})
      child.stdin.end(input ?? '')
    })
  }

  /** One bounded, readable form of a failure for stderr. */
  const describe = (error) => String(error && error.stack ? error.stack.split('\n').slice(0, 4).join(' | ') : error).slice(0, 600)

  /**
   * Capture one ended run.
   *
   * The read starts synchronously from the caller: an experiment plugin issuing
   * the same `readSession` from a `turn/end` listener succeeds, while this
   * bridge's first successful call is still unexplained under the headless
   * runner's teardown, so the read must begin at the earliest possible point.
   * @param sessionId - session whose run to capture.
   * @param turn - the exact turn.
   * @param read - a function returning the session read; the first call's
   *   promise is already in flight (see the caller), a retry starts a fresh one.
   */
  function captureTurn(sessionId, turn, read, attemptsLeft = CAPTURE_ATTEMPTS) {
    // The read goes through the live `ctx` property, never a captured service
    // proxy, which loses its owning context (`cannot get required service ...
    // in inactive context`).
    publishTurn(stateDir, sessionId, turn, read)
      .then(async (published) => {
        if (published === null) return
        const { path, cwd } = published
        const result = await run([
          '--state-dir', stateDir, 'dsh', '--dsh-home', dshHome,
          'capture', '--session', path, '--project', cwd, '--turn', String(turn),
        ])
        if (result.code !== 0) {
          warn(`capture failed for turn ${turn}: ${result.output.slice(0, 500)}`)
          return
        }
        const report = parseReport(result.output)
        if (retryable(report.reason)) {
          // Nothing else drives a DSH run. The engine defers for longer than
          // this retry, so an early attempt reports `no_ready_run`; either
          // reason may still become captureable, and the projection must stay.
          if (attemptsLeft > 1) {
            // A retry needs a fresh snapshot: the harness flushes over time.
            const timer = setTimeout(
              () => captureTurn(sessionId, turn, () => ctx.sessionQuery.readSession(sessionId), attemptsLeft - 1),
              CAPTURE_RETRY_MS,
            )
            // Do not hold the harness process open merely to retry: if teardown
            // wins, the run stays queued for the documented `reconcile`.
            timer.unref?.()
            return
          }
          warn(`capture still ${report.reason} for turn ${turn}; left to reconcile`)
          return
        }
        if (report.state === undefined) {
          // An unparsable report: keep the projection available for reconcile
          // rather than deleting the only snapshot of a queued run.
          warn(`capture reported no run for turn ${turn}; left to reconcile`)
          return
        }
        // Every other result is terminal: evidence, a hold, or an exclusion.
        await removeProjection(path)
      })
      .catch((error) => warn(`capture deferred for turn ${turn}: ${describe(error)}`))
  }

  /** Arm the latch for one session so capture waits for that compaction. */
  const armCompaction = (sessionId) => {
    if (compacting.has(String(sessionId))) return
    let finish
    const done = new Promise(resolve => { finish = resolve })
    compacting.set(String(sessionId), { done, finish })
  }
  for (const marker of COMPACTION_REQUEST_EVENTS) {
    ctx.on(marker, (payload) => {
      if (payload !== null && typeof payload === 'object' && 'sessionId' in payload) {
        armCompaction(payload.sessionId)
      }
    })
  }

  // A serial listener: the agent factory awaits it during creation, so the
  // per-agent observer exists before the queued prompt opens its first turn.
  // The observer is owned by this agent's scope, so it ends with the agent.
  ctx.on('agent/created', ({ agent }) => {
    const session = agent.session
    if (engineOwned(session.id)) return
    agent.ctx.effect(() => {
      const stop = agent.ctx.on('session/event', (subject, event) => {
        if (subject !== session) return
        if (event.type === 'compaction/start') {
          armCompaction(session.id)
          return
        }
        if (event.type === 'compaction/end') {
          compacting.get(String(session.id))?.finish()
          compacting.delete(String(session.id))
          return
        }
        if (event.type !== 'turn/end') return
        const header = subject.header
        if (header.isSeeded === true || header.origin === 'subagent' || (header.delegationDepth ?? 0) > 0) return
        if (header.cwd === undefined) return
        const turn = event.data.turn
        const signal = JSON.stringify({
          kind: 'turn-end',
          session: session.id,
          turn,
          cwd: header.cwd,
          reason: event.data.reason.kind,
        })
        if (Buffer.byteLength(signal) > MAX_SIGNAL_BYTES) return
        // The read starts one microtask past the commit: starting inside the
        // commit, or after the hint subprocess exits near teardown, both fail
        // (see the experiment report). Compaction intent is rechecked in that
        // microtask, because a producer whose `turn/end` listener runs after
        // this bridge's announces it later in the same dispatch. A failed read
        // leaves the hinted run queued for a documented reconcile.
        const started = Promise.resolve()
          .then(() => waitForCompaction(session.id))
          .then(() => ctx.sessionQuery.readSession(session.id))
        const read = (() => {
          let consumed = false
          return () => {
            if (consumed) return ctx.sessionQuery.readSession(session.id)
            consumed = true
            return started
          }
        })()
        started.catch(() => {})
        run(['--state-dir', stateDir, 'dsh', '--dsh-home', dshHome, '--event', signal, 'enqueue'])
          .then((queued) => {
            if (queued.code !== 0) {
              warn(`enqueue failed for turn ${turn}: ${queued.output.slice(0, 500)}`)
              return
            }
            const hint = parseReport(queued.output)
            if (hint.state === 'deferred') {
              // The project is not registered yet: the hint is kept, but no run
              // is queued, so capture would only report `no_ready_run`. The
              // projection is still published, because `register` and the
              // following `reconcile` consume it to establish the baseline and
              // resolve this hint; no capture invocation would write it later.
              publishTurn(stateDir, session.id, turn, read)
                .catch((error) => warn(`deferred projection failed for turn ${turn}: ${describe(error)}`))
              warn(`turn ${turn} is queued for later (${hint.reason ?? 'project not registered'}); run reconcile after registering the project`)
              return
            }
            captureTurn(session.id, turn, read)
          })
          .catch((error) => warn(`hint/capture failed: ${String(error && error.stack ? error.stack.split('\n').slice(0, 4).join(' | ') : error).slice(0, 600)}`))
      })
      return () => stop()
    }, 'coding-memory-capture.observe')
  })
}
