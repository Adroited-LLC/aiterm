package com.adroited.aiterm.ui

import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.remote.ConnectionState
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class RemoteAppDrawerTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()

    @Test
    fun terminalRemainsAccessibleFromTheDashboardToolbar() {
        val desktop = PairedDesktop(
            deviceId = "desktop-1",
            displayName = "Workshop PC",
            hosts = listOf("10.0.0.151"),
            port = 8443,
            serverSpkiFingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            lastSeenEpochMillis = null,
        )
        var terminalRequests = 0
        compose.setContent {
            RemoteSessionDashboard(
                state = RemoteClientState(connection = ConnectionState.Connected),
                desktop = desktop,
                pairedDesktops = listOf(desktop),
                onOpenDesktop = {},
                onManageDesktops = {},
                onRefresh = {},
                onLoadUsage = {},
                onStarSession = { _, _ -> },
                onRenameSession = { _, _ -> },
                onOpenSession = {},
                onOpenTerminal = { terminalRequests++ },
            )
        }
        compose.onNodeWithContentDescription("Open terminal").assertIsDisplayed().performClick()
        compose.runOnIdle { assertEquals(1, terminalRequests) }
    }

    @Test
    fun usageCollapsesAndDesktopManagementRemainsTheSingleManagementEntry() {
        val desktop = PairedDesktop(
            deviceId = "desktop-1",
            displayName = "Workshop PC",
            hosts = listOf("10.0.0.151"),
            port = 8443,
            serverSpkiFingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            lastSeenEpochMillis = null,
        )
        var managementRequests = 0
        compose.setContent {
            RemoteAppDrawer(
                state = RemoteClientState(),
                desktop = desktop,
                pairedDesktops = listOf(desktop),
                onClose = {},
                onOpenDesktop = {},
                onLoadUsage = {},
                onManageDesktops = { managementRequests++ },
            )
        }

        compose.onNodeWithText("Open terminal").assertDoesNotExist()
        compose.onNodeWithText("Refresh").assertDoesNotExist()
        compose.onNodeWithText("Add a desktop").assertDoesNotExist()
        compose.onNodeWithText("Forget this desktop").assertDoesNotExist()
        compose.onNodeWithText("Reading account limits…").assertDoesNotExist()
        compose.onNodeWithText("Usage").performClick()
        compose.onNodeWithText("Reading account limits…").assertIsDisplayed()
        compose.onNodeWithText("Usage").performClick()
        compose.onNodeWithText("Reading account limits…").assertDoesNotExist()
        compose.onNodeWithText("Manage desktops").performClick()
        compose.runOnIdle { assertEquals(1, managementRequests) }
    }

    @Test
    fun friendlyNamesAppearInDrawerHeaderAndDesktopSwitcher() {
        val current = PairedDesktop(
            deviceId = "desktop-1",
            displayName = "WORKSHOP-123",
            hosts = listOf("10.0.0.151"),
            port = 8443,
            serverSpkiFingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            lastSeenEpochMillis = null,
            friendlyName = "Home office",
        )
        val other = current.copy(
            deviceId = "desktop-2",
            displayName = "WORKSTATION-456",
            friendlyName = "Work PC",
        )
        var openedDesktop: PairedDesktop? = null
        compose.setContent {
            RemoteAppDrawer(
                state = RemoteClientState(),
                desktop = current,
                pairedDesktops = listOf(current, other),
                onClose = {},
                onOpenDesktop = { openedDesktop = it },
                onLoadUsage = {},
                onManageDesktops = {},
            )
        }

        compose.onAllNodesWithText("Home office").assertCountEquals(2)
        compose.onNodeWithText("Work PC").assertIsDisplayed().performClick()
        compose.runOnIdle { assertEquals(other, openedDesktop) }
    }

}
