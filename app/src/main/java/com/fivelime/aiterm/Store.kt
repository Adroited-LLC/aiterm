package com.fivelime.aiterm

import android.content.Context

/** The one desktop this phone is paired with. */
data class Desktop(
    val baseUrl: String, val token: String, val name: String,
    val candidates: List<String> = listOf(baseUrl),
    /** SHA-256 of the desktop's certificate, hex. The only thing we trust. */
    val fingerprint: String = "",
) {
    /** The address that answered last, then the rest in the QR's order. */
    val ordered: List<String> get() = listOf(baseUrl) + candidates.filter { it != baseUrl }
}

/** Private app storage. The token is the only secret; it never leaves here
 *  except as a request header. */
class Store(context: Context) {
    private val prefs = context.getSharedPreferences("aiterm", Context.MODE_PRIVATE)
    /** Preferences live apart from pairing, so forgetting a desktop keeps
     *  the person's theme and time zone. */
    private val settings = context.getSharedPreferences("aiterm.settings", Context.MODE_PRIVATE)

    var theme: String
        get() = settings.getString("theme", "dark") ?: "dark"
        set(v) { settings.edit().putString("theme", v).apply() }

    /** IANA zone id; empty = the phone's own zone. */
    var timeZone: String
        get() = settings.getString("tz", "") ?: ""
        set(v) { settings.edit().putString("tz", v).apply() }

    /** Masters whose brought-in crew is folded away in the list. */
    var foldedCrews: Set<String>
        get() = settings.getStringSet("folded_crews", emptySet()) ?: emptySet()
        set(v) { settings.edit().putStringSet("folded_crews", v).apply() }

    /** Require fingerprint (or device credential) to open the app. */
    var biometric: Boolean
        get() = settings.getBoolean("biometric", false)
        set(v) { settings.edit().putBoolean("biometric", v).apply() }

    fun load(): Desktop? {
        val url = prefs.getString("url", null) ?: return null
        val token = prefs.getString("token", null) ?: return null
        val candidates = prefs.getString("urls", null)?.split('\n')?.filter { it.isNotBlank() } ?: listOf(url)
        val fp = prefs.getString("fp", null) ?: return null // paired before pinning existed: pair again
        return Desktop(url, token, prefs.getString("name", null) ?: "Desktop", candidates, fp)
    }

    fun save(d: Desktop) {
        prefs.edit().putString("url", d.baseUrl).putString("token", d.token).putString("name", d.name)
            .putString("urls", d.candidates.joinToString("\n")).putString("fp", d.fingerprint).apply()
    }

    fun clear() = prefs.edit().clear().apply()
}
