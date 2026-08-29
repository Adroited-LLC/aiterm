package com.adroited.aiterm.ui

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LockedContentTest {

    @get:Rule val compose = createComposeRule()

    @Test
    fun lockedApp_hidesDesktopDataUntilAuthenticationStarts() {
        var requestedUnlock = false
        compose.setContent { LockedContent(onUnlock = { requestedUnlock = true }) }

        compose.onNodeWithText("AITerm is locked").assertIsDisplayed()
        compose.onNodeWithText("Unlock with a strong biometric or your device PIN.")
            .assertIsDisplayed()
        compose.onNodeWithText("Unlock AITerm").performClick()

        assertTrue(requestedUnlock)
    }
}
