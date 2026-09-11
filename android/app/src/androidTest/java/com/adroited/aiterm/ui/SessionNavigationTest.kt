package com.adroited.aiterm.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.remote.*
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SessionNavigationTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()
    private fun session(id: String, title: String) = RemoteSession(id, "codex", title,
        "/work/project", "/work/project", forked = false, background = false, lastActive = 0)

    @Test fun titleDropdownOffersOnlyLiveSessionsAndSelectsTheRequestedOne() {
        val current = session("a", "Current work")
        val target = session("b", "Other work")
        var selected: RemoteSession? = null
        val state = RemoteClientState(sessions = listOf(current, target,
            session("c", "Closed work"), session("d", "Historical work")), tabs = listOf(
            RemoteTab("tab-a", "A", sessionId = "a", size = TerminalSize(80, 24)),
            RemoteTab("tab-b", "B", sessionId = "b", size = TerminalSize(80, 24)),
            RemoteTab("tab-c", "C", sessionId = "c", size = TerminalSize(80, 24), state = RemoteTabState.Exited),
        ))
        compose.setContent { MaterialTheme {
            SessionSwitcher(state, current, onSelectSession = { selected = it }) { Text("working") }
        } }
        compose.onNodeWithText("Current work").performClick()
        compose.onNodeWithText("Live sessions").assertIsDisplayed()
        compose.onNodeWithContentDescription("Selected session").assertIsDisplayed()
        compose.onNodeWithText("Closed work").assertDoesNotExist()
        compose.onNodeWithText("Historical work").assertDoesNotExist()
        compose.onNodeWithText("Other work").performClick()
        compose.runOnIdle { assertEquals(target, selected) }
        compose.onNodeWithText("Live sessions").assertDoesNotExist()
    }

    @Test fun apiHeaderSwitchesSessionsWithoutLosingDraftOrShowingAnotherSessionsPendingPrompt() {
        val first = session("a", "First session")
        val second = session("b", "Second session")
        val selected = mutableStateOf(first)
        val drafts = ConversationDraftStore()
        val state = RemoteClientState(connection = ConnectionState.Connected,
            sessions = listOf(first, second), tabs = listOf(
                RemoteTab("tab-a", "A", sessionId = "a", size = TerminalSize(80, 24)),
                RemoteTab("tab-b", "B", sessionId = "b", size = TerminalSize(80, 24))),
            pendingPrompts = listOf(PendingConversationPrompt("p", "a", "Check the queued message", true)))
        compose.setContent { MaterialTheme {
            androidx.compose.runtime.key(selected.value.id) {
                RemoteConversationContent(state = state, session = selected.value,
                    onBack = {}, onRefresh = {}, onSend = { _, _, _, _ -> Result.success(Unit) },
                    onBringIn = { _, _, _, _, _, _, _ -> }, onStar = { _, _ -> },
                    onOpen = {}, onOpenTerminal = {}, onStop = {},
                    onLoadFiles = { Result.success(emptyList()) },
                    onLoadFile = { _, _, _ -> error("Unexpected file") },
                    onParseMarkdown = { Result.success(RemoteMarkdownDocument(emptyList())) },
                    onSaveMarkdown = { _, _, _, _ -> error("Unexpected save") },
                    onRenderSvg = { _, _ -> error("Unexpected SVG") },
                    onProbeWebPreview = { Result.success(false) },
                    onOpenWebPreview = { error("Unexpected web preview") }, onShowWebPreview = {},
                    onSelectSession = { selected.value = it }, onQuickInput = { _, _ -> },
                    conversationDraftStore = drafts)
            }
        } }
        compose.onNodeWithText("Check the queued message").assertIsDisplayed()
        compose.onNode(hasSetTextAction()).performTextInput("Keep my draft")
        compose.onNodeWithText("First session").performClick()
        compose.waitForIdle()
        val bitmap = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot()
        java.io.File(compose.activity.cacheDir, "session-switcher.png").outputStream().use {
            bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
        }
        bitmap.recycle()
        compose.onNodeWithText("Second session").performClick()
        compose.onNodeWithText("Check the queued message").assertDoesNotExist()
        compose.onNodeWithText("Keep my draft").assertDoesNotExist()
        compose.onNodeWithText("First session").assertDoesNotExist()
        compose.onNodeWithText("Second session").performClick()
        compose.onNodeWithText("First session").performClick()
        compose.onNode(hasSetTextAction()).assertTextContains("Keep my draft")
        compose.onNodeWithText("Check the queued message").assertIsDisplayed()
    }

    @Test fun statusChangesPreserveMessageAndComposerPositionsAtTailAndInHistory() {
        val session = session("stable", "Stable conversation")
        val state = mutableStateOf(RemoteClientState(
            connection = ConnectionState.Connected,
            sessions = listOf(session),
            tabs = listOf(RemoteTab("tab", "Stable", sessionId = session.id, size = TerminalSize(80, 24))),
            previewSessionId = session.id, previewLive = true, previewTurnOpen = true,
            previewPhase = SpinePhase.Working,
            previewItems = (0..79).map { Item.User("message-$it", "Message $it", it.toLong()) },
        ))
        compose.setContent { MaterialTheme {
            RemoteConversationContent(state = state.value, session = session,
                onBack = {}, onRefresh = {}, onSend = { _, _, _, _ -> Result.success(Unit) },
                onBringIn = { _, _, _, _, _, _, _ -> }, onStar = { _, _ -> },
                onOpen = {}, onOpenTerminal = {}, onStop = {},
                onLoadFiles = { Result.success(emptyList()) },
                onLoadFile = { _, _, _ -> error("Unexpected file") },
                onParseMarkdown = { Result.success(RemoteMarkdownDocument(emptyList())) },
                onSaveMarkdown = { _, _, _, _ -> error("Unexpected save") },
                onRenderSvg = { _, _ -> error("Unexpected SVG") },
                onProbeWebPreview = { Result.success(false) },
                onOpenWebPreview = { error("Unexpected web preview") }, onShowWebPreview = {},
                onSelectSession = {}, onQuickInput = { _, _ -> })
        } }
        fun checkAnchor(text: String) {
            compose.onNodeWithText(text).assertIsDisplayed()
            val message = compose.onNodeWithText(text).fetchSemanticsNode().boundsInRoot
            val composer = compose.onNode(hasSetTextAction()).fetchSemanticsNode().boundsInRoot
            repeat(4) { index ->
                compose.runOnIdle { state.value = state.value.copy(
                    previewPhase = if (index % 2 == 0) SpinePhase.NeedsYou else SpinePhase.Working,
                    previewPhaseDetail = if (index % 2 == 0) "permission: Bash" else "Running tests",
                ) }
                compose.waitForIdle()
                assertEquals(message, compose.onNodeWithText(text).fetchSemanticsNode().boundsInRoot)
                assertEquals(composer, compose.onNode(hasSetTextAction()).fetchSemanticsNode().boundsInRoot)
            }
            compose.runOnIdle { state.value = state.value.copy(previewTurnOpen = false, previewPhase = SpinePhase.Idle) }
            compose.waitForIdle()
            assertEquals(message, compose.onNodeWithText(text).fetchSemanticsNode().boundsInRoot)
            assertEquals(composer, compose.onNode(hasSetTextAction()).fetchSemanticsNode().boundsInRoot)
            compose.runOnIdle { state.value = state.value.copy(previewTurnOpen = true, previewPhase = SpinePhase.Working) }
        }
        checkAnchor("Message 79")
        compose.onNode(hasScrollToIndexAction()).performScrollToNode(hasText("Message 30"))
        checkAnchor("Message 30")
    }

    @Test fun pendingCardShowsAcceptanceAndConnectionStateUntilConfirmed() {
        val prompts = mutableStateOf(listOf(PendingConversationPrompt("p", "a", "Please check this next")))
        val connected = mutableStateOf(true)
        compose.setContent { MaterialTheme {
            PendingPromptCards(prompts.value, connected.value, onHide = { id ->
                prompts.value = prompts.value.filterNot { it.id == id }
            })
        } }
        compose.onNodeWithText("Please check this next").assertIsDisplayed()
        compose.onNodeWithText("Sending to desktop…").assertIsDisplayed()
        compose.runOnIdle { prompts.value = prompts.value.map { it.copy(accepted = true) } }
        compose.onNodeWithText("Sent · waiting for agent").assertIsDisplayed()
        compose.runOnIdle { connected.value = false }
        compose.onNodeWithText("Sent · waiting for connection to confirm").assertIsDisplayed()
        compose.onNodeWithText("Please check this next").assertIsDisplayed()
        compose.runOnIdle { prompts.value = emptyList() }
        compose.onNodeWithText("Pending prompt").assertDoesNotExist()
    }
}
