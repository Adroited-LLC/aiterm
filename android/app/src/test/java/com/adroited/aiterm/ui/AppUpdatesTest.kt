package com.adroited.aiterm.ui
import org.junit.Assert.*
import org.junit.Test
class AppUpdatesTest {
    private val valid = AndroidUpdatePackage("0.3.17", 20, "aiterm.apk", "a".repeat(64), 100)
    @Test fun oldGithubCredentialsCannotBeSentToUpdateService() {
        assertTrue(validUpdateInvite("aiterm_" + "x".repeat(43)))
        assertFalse(validUpdateInvite("github_pat_secret"))
        assertFalse(validUpdateInvite("aiterm_short"))
    }
    @Test fun validPrivateReleasePackageIsAccepted() { validateAndroidUpdate(valid) }
    @Test fun rejectsTraversalAndInvalidVerificationData() {
        listOf(valid.copy(asset = "../aiterm.apk"), valid.copy(asset = "x\\aiterm.apk"), valid.copy(asset = "aiterm.exe"),
            valid.copy(sha256 = "a"), valid.copy(size = 0), valid.copy(size = Long.MAX_VALUE), valid.copy(versionCode = -1), valid.copy(version = "latest"))
            .forEach { assertThrows(IllegalArgumentException::class.java) { validateAndroidUpdate(it) } }
    }
}
