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

    @Test
    fun dashboardFiltersComposeAndStarsStayFirst() {
        val sessions = listOf(
            session("claude", "Claude", agent = "claude", lastActive = 30),
            session("live", "Live", lastActive = 20),
            session("star", "Star", lastActive = 10),
        )

        assertEquals(
            listOf("star", "live", "claude"),
            conversationSessions(sessions, listOf(liveTab), "", starred = setOf("star")).map { it.id },
        )
        assertEquals(
            listOf("live"),
            conversationSessions(sessions, listOf(liveTab), "", activeOnly = true).map { it.id },
        )
        assertEquals(
            listOf("claude"),
            conversationSessions(sessions, listOf(liveTab), "", agentFilter = "claude").map { it.id },
        )
        assertEquals(
            listOf("star"),
            conversationSessions(sessions, listOf(liveTab), "", withFiles = setOf("star"), filesOnly = true)
                .map { it.id },
        )
    }

    @Test
    fun broughtInSessionsSitBelowTheirMasterAndCanBeFolded() {
        val sessions = listOf(
            session("child", "Second agent", lastActive = 30),
            session("other", "Other", lastActive = 20),
            session("master", "Main work", lastActive = 10),
        )
        val lineage = mapOf("child" to "master")

        assertEquals(
            listOf("other", "master", "child"),
            conversationSessions(sessions, emptyList(), "", broughtIn = lineage).map { it.id },
        )
        assertEquals(
            listOf("other", "master"),
            conversationSessions(
                sessions,
                emptyList(),
                "",
                broughtIn = lineage,
                foldedCrews = setOf("master"),
            ).map { it.id },
        )
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
