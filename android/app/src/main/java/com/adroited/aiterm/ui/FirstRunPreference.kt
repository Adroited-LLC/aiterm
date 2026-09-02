package com.adroited.aiterm.ui

import android.content.Context
import android.content.SharedPreferences

/** Non-secret, installation-local record that the welcome screen was completed. */
class FirstRunPreference(private val preferences: SharedPreferences) {
    constructor(context: Context) : this(
        context.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE),
    )

    fun shouldShowWelcome(): Boolean = !preferences.getBoolean(COMPLETED_KEY, false)

    fun completeWelcome(): Boolean = preferences.edit().putBoolean(COMPLETED_KEY, true).commit()

    private companion object {
        const val PREFERENCES_NAME = "first_run"
        const val COMPLETED_KEY = "welcome_completed"
    }
}
