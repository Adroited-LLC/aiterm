package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.*
import org.junit.Assert.*
import org.junit.Test

class SessionMenuActionsTest {
    private val session = RemoteSession("s", "codex", "Work", "/work", "/work", forked = false, background = false, lastActive = 0)
    private val caps = RemoteAgentCaps(true, false, true, true, false, false, true, false, false)
    private val state = RemoteClientState(agentCaps = mapOf("codex" to caps))
    private val tab = RemoteTab("t", "Work", sessionId = "s", size = TerminalSize(80, 24))
    @Test fun inactiveSessionsCanResumeForkAndDelete() {
        val actions = sessionMenuActions(state, session)
        assertTrue(actions.containsAll(listOf(SessionMenuAction.Resume, SessionMenuAction.Fork, SessionMenuAction.Delete)))
        assertFalse(SessionMenuAction.Close in actions)
    }
    @Test fun liveSessionsCanCloseButCannotDeleteOrResumeTheRunningProcess() {
        val actions = sessionMenuActions(state.copy(tabs = listOf(tab)), session)
        assertTrue(SessionMenuAction.Close in actions)
        assertFalse(SessionMenuAction.Delete in actions)
        assertFalse(SessionMenuAction.Resume in actions)
    }
    @Test fun exitedTabsAreEligibleForResumeAndDeletion() {
        assertTrue(SessionMenuAction.Delete in sessionMenuActions(state.copy(tabs = listOf(tab.copy(state = RemoteTabState.Exited))), session))
    }
    @Test fun activityProtectsSessionsDuringTabDiscoveryGaps() {
        val actions = sessionMenuActions(state.copy(sessionActivity = mapOf("s" to "output")), session)
        assertTrue(SessionMenuAction.Stop in actions)
        assertFalse(SessionMenuAction.Delete in actions)
    }
    @Test fun unsupportedAgentActionsAreNotOffered() {
        val actions = sessionMenuActions(state.copy(agentCaps = emptyMap()), session)
        assertFalse(SessionMenuAction.Fork in actions)
        assertFalse(SessionMenuAction.Resume in actions)
        assertFalse(SessionMenuAction.Delete in actions)
        assertTrue(SessionMenuAction.Open in actions)
        assertTrue(SessionMenuAction.Rename in actions)
    }
}
