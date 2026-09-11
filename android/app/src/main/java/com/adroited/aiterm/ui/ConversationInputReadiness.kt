package com.adroited.aiterm.ui

import com.adroited.aiterm.terminal.ScreenSnapshot
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull

/** An attached PTY can still be blank, or displaying the CLI's startup composer. */
internal suspend fun awaitConversationInputScreen(
    screens: Flow<ScreenSnapshot?>,
    tabId: String,
    agentId: String?,
): ScreenSnapshot? = withTimeoutOrNull(10_000) {
    screens.filterNotNull().first { screen ->
        screen.tabId == tabId && screen.readyForConversationInput(agentId)
    }
}

internal fun ScreenSnapshot.readyForConversationInput(agentId: String?): Boolean {
    val lines = visible.map { it.plainText() }
    if (lines.none { it.isNotBlank() }) return false
    if (agentId != "codex") return true
    // Codex enables bracketed paste before drawing its provisional startup composer.
    // That composer accepts text but cannot submit it yet. Wait for initialization,
    // and never send a multiline message as raw keystrokes during that transition.
    return modes.bracketedPaste && lines.none { CODEX_LOADING_HEADER.containsMatchIn(it) }
}

private val CODEX_LOADING_HEADER = Regex("^\\s*[│|]?\\s*(?:model|directory):\\s+loading(?:\\s|$)")
