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
        compose.onNodeWithText("GitHub access token").assertIsDisplayed()
        compose.onNodeWithText("Connect").assertIsNotEnabled()
        compose.waitUntil(5000) { compose.onAllNodesWithText("Connect your update access first.").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText("Check for updates").performClick()
        compose.waitUntil(5000) { compose.onAllNodesWithText("Connect your update access first.").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText("Close").performClick()
        compose.onNodeWithText("GitHub access token").assertDoesNotExist()
    }
}
