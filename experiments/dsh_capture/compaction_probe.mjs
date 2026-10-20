/**
 * Experiment-only compaction driver for issue #56.
 *
 * After the headless runner's first turn settles, this plugin asks the mounted
 * compaction backend to condense the conversation with the same code path the
 * `/compact` command uses (`ctx.compaction.compactNow`). It is never part of the
 * production bridge; it exists only to produce a real compacted session log for
 * the capture experiment.
 *
 * @module coding-memory-compaction-probe
 */

/** Cordis plugin name. */
export const name = 'coding-memory-compaction-probe'
/** Services required to find the agent and condense its history. */
export const inject = ['commands']
/** Marker a bridge reads to wait for the compaction this probe is about to run. */
export const COMPACTION_REQUEST = 'coding-memory/compaction-requested' 

/**
 * Mount the driver.
 * @param ctx - Cordis context with the command registry available.
 * @param config - `triggerTurn` is the turn number after which to compact.
 */
export function apply(ctx, config) {
  const triggerTurn = config?.triggerTurn ?? 1
  /** Events observed this process, used only to report the compaction outcome. */
  const events = []
  const log = (message) => process.stderr.write(`compaction-probe: ${message}\n`)
  let done = false

  ctx.on('agent/created', ({ agent }) => {
    if (done) return
    agent.ctx.effect(() => {
      const stop = agent.ctx.on('session/event', (session, event) => {
        events.push(event)
        if (done || event.type !== 'turn/end' || event.data.turn !== triggerTurn) return
        done = true
        // Announce the impending compaction synchronously, before the capture
        // that the bridge starts one microtask later, so that capture can wait
        // for it instead of reading the pre-compaction log.
        agent.ctx.emit(COMPACTION_REQUEST, { sessionId: String(session.id) })
        // The `/compact` invocation needs an idle agent: the turn is committed
        // but the runner may still report `running` for a moment.
        Promise.resolve().then(async () => {
          try {
            if (typeof agent.whenIdle === 'function') await agent.whenIdle()
            const execution = await ctx.commands.execute(agent, '/compact', [], new AbortController().signal)
            const result = execution?.result
            log(`compact result kind=${result?.kind} text=${String(result?.text ?? '').slice(0, 200)}`)
            // The bracket records the summarization failure; report it too so
            // the measurement names its cause without re-reading the log.
            try {
              const failures = events
                .filter(event => event.type === 'compaction/end' && event.data.error !== undefined)
                .map(event => String(event.data.error).slice(0, 300))
              if (failures.length > 0) log(`compaction errors: ${failures.join(' ; ')}`)
            } catch (error) {
              log(`cannot read compaction errors: ${String(error)}`)
            }
          } catch (error) {
            log(`compact failed: ${String(error)}`)
          }
        })
      })
      return () => stop()
    }, 'coding-memory-compaction-probe.observe')
  })
}
