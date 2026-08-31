package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.RemoteSession
import com.adroited.aiterm.remote.RemoteTab
import com.adroited.aiterm.remote.TerminalSize
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteConversationScreenTest {
    private val liveTab = RemoteTab(
        id = "tab-live",
        title = "Live terminal",
        sessionId = "live",
        size = TerminalSize(80, 24),
    )

    @Test
    fun liveSessionsLeadTheDashboardAndRecentSessionsFollow() {
        val sessions = listOf(
            session("old", "Older", lastActive = 10),
            session("live", "Live", lastActive = 5),
            session("new", "Newer", lastActive = 20),
        )

        assertEquals(
            listOf("live", "new", "old"),
            conversationSessions(sessions, listOf(liveTab), "").map { it.id },
        )
    }

    @Test
    fun dashboardSearchUsesTitleAgentAndProjectWithoutCaseSensitivity() {
        val sessions = listOf(
            session("one", "Release prep", agent = "codex", project = "/work/aiterm"),
            session("two", "Notes", agent = "claude", project = "/work/docs"),
        )

        assertEquals(listOf("one"), conversationSessions(sessions, emptyList(), "AITERM").map { it.id })
        assertEquals(listOf("two"), conversationSessions(sessions, emptyList(), "CLAUDE").map { it.id })
        assertEquals(listOf("one"), conversationSessions(sessions, emptyList(), "release").map { it.id })
    }

    @Test
    fun liveStateComesOnlyFromARealTabForThatSession() {
        assertTrue(isConversationSessionLive(session("live", "Live"), listOf(liveTab)))
        assertFalse(isConversationSessionLive(session("other", "Other"), listOf(liveTab)))
    }

    private fun session(
        id: String,
        title: String,
        agent: String = "codex",
        project: String = "/work/project",
        lastActive: Long = 0,
    ) = RemoteSession(
        id = id,
        agent = agent,
        title = title,
        projectPath = project,
        groupPath = project,
        branch = null,
        forked = false,
        background = false,
        forkParent = null,
        lastActive = lastActive,
    )
}
