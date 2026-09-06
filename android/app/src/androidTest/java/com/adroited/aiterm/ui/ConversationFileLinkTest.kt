package com.adroited.aiterm.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.UriHandler
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.click
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.text.TextLayoutResult
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.remote.Item
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.remote.RemoteMarkdownBlock
import com.adroited.aiterm.remote.RemoteMarkdownDocument
import com.adroited.aiterm.remote.RemoteMarkdownSpan
import com.adroited.aiterm.remote.RemoteSession
import com.adroited.aiterm.remote.RemoteSessionChange
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ConversationFileLinkTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()

    @Test
    fun conversationFileTapLoadsRemotePreviewAndReturnsToConversation() {
        showConversation()

        tapLink(label)
        compose.onNodeWithText("README.md").assertIsDisplayed()
        compose.onNodeWithText(contents).assertIsDisplayed()
        compose.runOnIdle {
            assertEquals(listOf(Triple("file-link-test", path, 512 * 1024)), requests)
            assertTrue(externalLinks.isEmpty())
        }
        compose.onNodeWithContentDescription("Back to conversation").performClick()
        compose.onNodeWithText(label).assertIsDisplayed()
        compose.onNodeWithText(contents).assertDoesNotExist()
        compose.runOnIdle { assertEquals(0, leaveConversation) }
    }

    @Test
    fun unavailableFileShowsErrorWithoutLoadingOrLaunchingAndroidIntent() {
        showConversation(fileAvailable = false)
        tapLink(label)
        compose.onNodeWithText("This file is not available in this session's files: $path").assertIsDisplayed()
        compose.runOnIdle {
            assertTrue(requests.isEmpty())
            assertTrue(externalLinks.isEmpty())
        }
        compose.onNodeWithText("OK").performClick()
        compose.onNodeWithText(label).assertIsDisplayed()
    }

    @Test
    fun angleBracketFileLinkWithSpacesAndLineNumberOpensHostPath() {
        val opened = mutableListOf<String>()
        val externalLinks = mutableListOf<String>()
        val errors = mutableListOf<String>()
        compose.setContent {
            CompositionLocalProvider(LocalUriHandler provides recordingUriHandler(externalLinks)) {
                MaterialTheme {
                    ConversationLinkHandler(onOpenFile = opened::add, onError = errors::add) {
                        ConversationMarkdown("[Open report](</home/matt/Projects/mojo/My Report.md:12:3>)")
                    }
                }
            }
        }
        tapLink("Open report")
        compose.runOnIdle {
            assertEquals(listOf("/home/matt/Projects/mojo/My Report.md"), opened)
            assertTrue(externalLinks.isEmpty())
            assertTrue(errors.isEmpty())
        }
    }

    @Test
    fun webLinkStillUsesParentUriHandler() {
        val opened = mutableListOf<String>()
        val externalLinks = mutableListOf<String>()
        val errors = mutableListOf<String>()
        val url = "https://example.com/docs?q=mojo"
        compose.setContent {
            CompositionLocalProvider(LocalUriHandler provides recordingUriHandler(externalLinks)) {
                MaterialTheme {
                    ConversationLinkHandler(onOpenFile = opened::add, onError = errors::add) {
                        ConversationMarkdown("[Documentation]($url)")
                    }
                }
            }
        }
        tapLink("Documentation")
        compose.runOnIdle {
            assertEquals(listOf(url), externalLinks)
            assertTrue(opened.isEmpty())
            assertTrue(errors.isEmpty())
        }
    }

    private val path = "/home/matt/Projects/mojo/README.md"
    private val label = "Open Mojo’s README"
    private val contents = "Mojo remote preview regression"
    private val requests = mutableListOf<Triple<String, String, Int>>()
    private val externalLinks = mutableListOf<String>()
    private var leaveConversation = 0

    private fun showConversation(fileAvailable: Boolean = true) {
        val session = RemoteSession(
            id = "file-link-test", agent = "codex", title = "File link test",
            projectPath = "/home/matt/Projects/mojo", groupPath = "/home/matt/Projects/mojo",
            forked = false, background = false, lastActive = 0,
        )
        compose.setContent {
            CompositionLocalProvider(LocalUriHandler provides recordingUriHandler(externalLinks)) {
                MaterialTheme {
                    RemoteConversationContent(
                        state = RemoteClientState(
                            connection = ConnectionState.Connected,
                            sessions = listOf(session), previewSessionId = session.id,
                            previewItems = listOf(Item.AgentText("reply", "[$label]($path)", true, 0)),
                        ),
                        session = session,
                        onBack = { leaveConversation++ }, onRefresh = {},
                        onSend = { _, _, _, _ -> Result.success(Unit) },
                        onBringIn = { _, _, _, _, _, _, _ -> },
                        onStar = { _, _ -> }, onOpen = {}, onOpenTerminal = {}, onStop = {},
                        onLoadFiles = {
                            Result.success(if (!fileAvailable) emptyList() else listOf(RemoteSessionChange(
                                path = path, name = "README.md", kind = "modified", at = 0,
                                sessionId = session.id, bytes = contents.encodeToByteArray().size.toLong(),
                            )))
                        },
                        onLoadFile = { sessionId, requestedPath, limit ->
                            requests += Triple(sessionId, requestedPath, limit)
                            Result.success(RemoteSessionFilePreview(
                                path = requestedPath, mime = "text/markdown",
                                total = contents.encodeToByteArray().size.toLong(),
                                data = contents.encodeToByteArray(), truncated = false,
                            ))
                        },
                        onParseMarkdown = { source ->
                            Result.success(RemoteMarkdownDocument(listOf(
                                RemoteMarkdownBlock("paragraph", spans = listOf(RemoteMarkdownSpan(source))),
                            )))
                        },
                        onSaveMarkdown = { _, _, _, _ -> error("Unexpected save") },
                        onRenderSvg = { _, _ -> error("Unexpected SVG render") },
                        onProbeWebPreview = { Result.success(false) },
                        onOpenWebPreview = { error("Unexpected web preview") },
                        onShowWebPreview = {}, onSelectSession = {}, onQuickInput = { _, _ -> },
                    )
                }
            }
        }

    }

    private fun recordingUriHandler(links: MutableList<String>) = object : UriHandler {
        override fun openUri(uri: String) { links += uri }
    }

    // Touch a glyph inside the link, including when its Text occupies a wider row.
    private fun tapLink(label: String) {
        val node = compose.onNodeWithText(label).assertIsDisplayed()
        val layouts = mutableListOf<TextLayoutResult>()
        node.performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
        val point = layouts.single().getBoundingBox(1).center
        node.performTouchInput { click(point) }
    }
}
