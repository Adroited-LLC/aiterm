package com.fivelime.aiterm

/** What the app did, kept where it can be read: every line goes to logcat
 *  under one tag — `adb logcat -s aiterm` — and into a ring the Settings
 *  screen can copy out. Records what the app did — a session opened, a
 *  message sent, where the transcript landed — never what anyone wrote. */
object Diag {
    const val TAG = "aiterm"
    private const val KEEP = 600
    private val ring = ArrayDeque<String>()

    @Synchronized
    fun log(area: String, msg: String) {
        android.util.Log.d(TAG, "[$area] $msg")
        ring.addLast("${stamp()} [$area] $msg")
        while (ring.size > KEEP) ring.removeFirst()
    }

    @Synchronized
    fun dump(): String = ring.joinToString("\n")

    @Synchronized
    fun clear() = ring.clear()

    private fun stamp(): String =
        java.text.SimpleDateFormat("HH:mm:ss.SSS", java.util.Locale.US).format(java.util.Date())
}
