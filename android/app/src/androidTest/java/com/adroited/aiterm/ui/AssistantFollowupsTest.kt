package com.adroited.aiterm.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.remote.RemoteSession
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AssistantFollowupsDeviceTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()
    @Test fun actionFillsTheRealComposerWithoutSubmittingOrDiscardingDraft() {
        val session = RemoteSession(id = "followup-test", agent = "codex", title = "Follow-up test", projectPath = "/fixture", groupPath = "/fixture", forked = false, background = false, lastActive = 0)
        val drafts = ConversationDraftStore()
        drafts.forSession(session.id).text = "My existing draft"
        var sent = 0
        compose.setContent {
            MaterialTheme {
                RemoteConversationContent(
                    state = RemoteClientState(connection = ConnectionState.Connected, sessions = listOf(session),
                        previewSessionId = session.id, previewItems = listOf(Item.AgentText("reply",
                            """Review complete.
- :codex-followup[Draft a reply]{prompt="Write a constructive reply."}""", true, 0))),
                    session = session, onBack = {}, onRefresh = {},
                    onSend = { _, _, _, _ -> sent++; Result.success(Unit) },
                    onBringIn = { _, _, _, _, _, _, _ -> }, onStar = { _, _ -> }, onOpen = {}, onOpenTerminal = {}, onStop = {},
                    onLoadFiles = { Result.success(emptyList()) }, onLoadFile = { _, _, _ -> error("Unexpected file") },
                    onParseMarkdown = { error("Unexpected document") }, onSaveMarkdown = { _, _, _, _ -> error("Unexpected save") },
                    onRenderSvg = { _, _ -> error("Unexpected image") }, onProbeWebPreview = { Result.success(false) },
                    onOpenWebPreview = { error("Unexpected preview") }, onShowWebPreview = {}, onSelectSession = {},
                    onQuickInput = { _, _ -> }, conversationDraftStore = drafts,
                )
            }
        }
        compose.onNodeWithText("Draft a reply").assertIsDisplayed().performClick()
        compose.onNodeWithText("My existing draft\n\nWrite a constructive reply.").assertIsDisplayed()
        compose.runOnIdle {
            assertEquals(0, sent)
            assertEquals("My existing draft\n\nWrite a constructive reply.", drafts.forSession(session.id).text)
        }
    }
}
