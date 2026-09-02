package com.adroited.aiterm.ui

import android.content.Context
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FirstRunPreferenceTest {
    @Test
    fun welcomeIsShownUntilItIsCompleted() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val preferences = context.getSharedPreferences("first-run-test", Context.MODE_PRIVATE)
        preferences.edit().clear().commit()
        try {
            assertTrue(FirstRunPreference(preferences).shouldShowWelcome())

            assertTrue(FirstRunPreference(preferences).completeWelcome())

            assertFalse(FirstRunPreference(preferences).shouldShowWelcome())
        } finally {
            preferences.edit().clear().commit()
        }
    }
}
