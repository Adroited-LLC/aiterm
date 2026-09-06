package com.adroited.aiterm.ui
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
@RunWith(AndroidJUnit4::class)
class AppUpdatesScreenTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()
    @Test fun privateUpdatesCanBeOpenedCheckedAndDismissedWithoutCredentials() {
        compose.setContent { MaterialTheme { AppUpdateButton(); AppUpdateHost() } }
        compose.onNodeWithText("App updates").performClick()
        compose.waitUntil(10000) { compose.onAllNodesWithText("Checking or downloading…").fetchSemanticsNodes().isEmpty() }
        if (compose.onAllNodesWithText("Private updates connected").fetchSemanticsNodes().isNotEmpty()) {
            compose.onNodeWithText("Invite code").assertDoesNotExist()
        } else {
            compose.onNodeWithText("Invite code").assertIsDisplayed()
            compose.onNodeWithText("Connect").assertIsNotEnabled()
        }
        compose.onNode(isToggleable()).performScrollTo().performClick().performClick()
        compose.onNodeWithText("Check for updates").performClick()
        compose.waitUntil(10000) { compose.onAllNodesWithText("Checking or downloading…").fetchSemanticsNodes().isEmpty() }
        compose.onNodeWithText("Close").performClick()
        compose.onNodeWithText("Invite code").assertDoesNotExist()
    }
    @Test fun connectedInviteIsHiddenAndCanBeChangedOrCancelled() {
        var submitted = ""
        var disconnected = false
        compose.setContent {
            MaterialTheme {
                AppUpdateAccess(true, false, { code, success -> submitted = code; success() }, { disconnected = true })
            }
        }
        compose.onNodeWithText("Private updates connected").assertIsDisplayed()
        compose.onNodeWithText("Invite code").assertDoesNotExist()
        compose.onNodeWithText("Change invite code").performClick()
        compose.onNodeWithText("Invite code").performTextInput("replacement")
        compose.onNodeWithText("Cancel").performClick()
        compose.onNodeWithText("Invite code").assertDoesNotExist()
        compose.onNodeWithText("Change invite code").performClick()
        compose.onNodeWithText("Save invite code").assertIsNotEnabled()
        compose.onNodeWithText("Invite code").performTextInput("replacement")
        compose.onNodeWithText("Save invite code").performClick()
        compose.onNodeWithText("Invite code").assertDoesNotExist()
        compose.runOnIdle { org.junit.Assert.assertEquals("replacement", submitted) }
        compose.onNodeWithText("Disconnect").performClick()
        compose.runOnIdle { org.junit.Assert.assertTrue(disconnected) }
    }
}
