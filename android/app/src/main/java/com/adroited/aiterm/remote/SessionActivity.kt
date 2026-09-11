package com.adroited.aiterm.remote

/** Turn boundaries outrank terminal repainting and timeout-based approval guesses. */
internal fun conversationPhase(
    phase: SpinePhase,
    detail: String,
    live: Boolean,
    turnOpen: Boolean?,
    rosterActivity: String? = null,
): SpinePhase {
    if (live && turnOpen == false) return SpinePhase.Idle
    // Older desktops infer these two reasons from a quiet transcript. A long
    // command or model request is not evidence of a question for the person.
    val inferredAttention = phase == SpinePhase.NeedsYou &&
        detail in setOf("approval", "a tool call is waiting")
    if (live && turnOpen == true) {
        return if (phase == SpinePhase.NeedsYou && !inferredAttention) phase else SpinePhase.Working
    }
    if (inferredAttention) return SpinePhase.Working
    if (!live && phase == SpinePhase.Idle && rosterActivity == "output") return SpinePhase.Working
    return phase
}

/** Separate status cursor: dashboard reads must never consume conversation history. */
internal class SessionActivity {
    var epoch = 0L; private set
    var latestSeq = 0L; private set
    private var phase = SpinePhase.Idle
    private var detail = ""
    private var live = false
    private var turnOpen: Boolean? = null

    val activity: String? get() {
        if (!live || turnOpen == null) return null
        return when (conversationPhase(phase, detail, live, turnOpen)) {
            SpinePhase.Idle -> "idle"
            SpinePhase.Working -> "output"
            SpinePhase.NeedsYou -> "attention"
        }
    }

    fun apply(page: SpineConversationPage) {
        if (epoch != page.epoch) {
            epoch = page.epoch
            latestSeq = 0
            phase = SpinePhase.Idle
            detail = ""
        }
        if (page.latestSeq < latestSeq) return
        if (turnOpen != page.turnOpen) { phase = SpinePhase.Idle; detail = "" }
        // Only a complete tail can describe the phase at the atomic turn gate.
        if (!page.hasMore) {
            page.events.sortedBy { it.seq }.forEach { event ->
                when (event.kind) {
                    "turn_started", "turn_ended", "reset" -> { phase = SpinePhase.Idle; detail = "" }
                    "phase" -> { phase = SpinePhase.from(event.phase); detail = event.detail.orEmpty() }
                }
            }
        }
        latestSeq = page.latestSeq
        live = page.live
        turnOpen = page.turnOpen
    }
}
