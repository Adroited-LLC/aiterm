package com.adroited.aiterm.ui

import android.app.Application
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.adroited.aiterm.testing.ComposeTestActivity
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Explicit opt-in device provisioning: no network or state change without the private fixture. */
@RunWith(AndroidJUnit4::class)
class ProvisionUpdateInviteTest {
    @get:Rule val compose = createAndroidComposeRule<ComposeTestActivity>()
    @Test fun connectProvisionedInviteThroughTheRealService() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        val fixture = File(app.cacheDir, "provision-update-invite")
        assumeTrue(fixture.isFile)
        val code = fixture.readText().trim()
        fixture.delete()
        lateinit var model: AppUpdateViewModel
        compose.runOnIdle { model = AppUpdateViewModel(app); model.connect(code) }
        compose.waitUntil(30000) { !model.state.value.busy }
        assertTrue(model.state.value.message, model.state.value.connected)
    }
}
