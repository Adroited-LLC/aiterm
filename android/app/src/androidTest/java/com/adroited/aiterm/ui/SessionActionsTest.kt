package com.adroited.aiterm.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.remote.*
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SessionActionsTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()
    private val session = RemoteSession("s", "codex", "Test conversation", "/work/project", "/work/project", forked = false, background = false, lastActive = 0)
    private val tab = RemoteTab("t", "Test", sessionId = "s", size = TerminalSize(80, 24))
    private val caps = RemoteAgentCaps(true, false, true, true, false, false, true, false, false)
    private val state = mutableStateOf(RemoteClientState(connection = ConnectionState.Connected, sessions = listOf(session), agentCaps = mapOf("codex" to caps)))
    private var opened = 0
    private val mutations = mutableListOf<Pair<String, RemoteSessionMutation>>()
    private var mutationResult: Result<Unit> = Result.success(Unit)
    private fun show(live: Boolean = false) {
        if (live) state.value = state.value.copy(tabs = listOf(tab))
        val desktop = PairedDesktop("desktop", "Workshop", listOf("10.0.0.1"), 8443,
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", null)
        compose.setContent { MaterialTheme {
            RemoteSessionDashboard(state.value, desktop, listOf(desktop), {}, {}, {}, {},
                { _, _ -> }, { _, _ -> }, { opened++ }, {},
                onMutateSession = { target, action -> mutations += target.id to action; mutationResult })
        } }
    }
    private fun hold() = compose.onNodeWithText(session.title).performTouchInput { longClick() }

    @Test fun normalTapStillOpensTheConversation() {
        show()
        compose.onNodeWithText(session.title).performClick()
        compose.runOnIdle { assertEquals(1, opened) }
        compose.onNodeWithText("Resume session").assertDoesNotExist()
    }
    @Test fun inactiveLongPressOffersActionsAndDeleteRequiresConfirmation() {
        show()
        hold()
        compose.onNodeWithText("Resume session").assertIsDisplayed()
        compose.onNodeWithText("Fork session").assertIsDisplayed()
        compose.onNodeWithText("Close session").assertDoesNotExist()
        compose.onNodeWithText("Delete session").performScrollTo().performClick()
        compose.onNodeWithText("Delete session?").assertIsDisplayed()
        compose.runOnIdle { assertEquals(0, opened); assertTrue(mutations.isEmpty()) }
        compose.onNodeWithText("Cancel").performClick()
        compose.runOnIdle { assertTrue(mutations.isEmpty()) }
        compose.onNodeWithText("Delete session").performScrollTo().performClick()
        compose.onAllNodesWithText("Delete session").onLast().performClick()
        compose.runOnIdle { assertEquals(listOf("s" to RemoteSessionMutation.Delete), mutations) }
        compose.onNodeWithText("Resume session").assertDoesNotExist()
    }
    @Test fun activeLongPressOffersCloseAndNeverDelete() {
        show(live = true)
        hold()
        compose.onNodeWithText("Close session").assertIsDisplayed()
        compose.onNodeWithText("Delete session").assertDoesNotExist()
        compose.onNodeWithText("Resume session").assertDoesNotExist()
        compose.onNodeWithText("Close session").performClick()
        compose.onNodeWithText("Close session?").assertIsDisplayed()
        compose.runOnIdle { assertTrue(mutations.isEmpty()) }
        compose.onNodeWithText("Cancel").performClick()
        compose.runOnIdle { assertTrue(mutations.isEmpty()) }
    }
    @Test fun becomingActiveInvalidatesAnOpenDeleteConfirmation() {
        show()
        hold()
        compose.onNodeWithText("Delete session").performScrollTo().performClick()
        compose.runOnIdle { state.value = state.value.copy(tabs = listOf(tab)) }
        compose.onNodeWithText("Delete session").assertIsNotEnabled()
        compose.runOnIdle { assertTrue(mutations.isEmpty()) }
    }
    @Test fun desktopErrorsRemainVisibleInTheMenu() {
        mutationResult = Result.failure(IllegalStateException("Desktop refused the fork"))
        show()
        hold()
        compose.onNodeWithText("Fork session").performClick()
        compose.onNodeWithText("Desktop refused the fork").assertIsDisplayed()
        compose.onNodeWithText("Rename session").assertIsDisplayed()
    }
}
