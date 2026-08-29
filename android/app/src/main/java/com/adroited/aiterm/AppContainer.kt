package com.adroited.aiterm

import android.content.Context
import com.adroited.aiterm.pairing.OkHttpPairingTransport
import com.adroited.aiterm.pairing.PairingRepository
import com.adroited.aiterm.pairing.SharedPreferencesPairedDesktopStore
import com.adroited.aiterm.pairing.PairedDesktopStoreException
import com.adroited.aiterm.security.AndroidDeviceKeyStore
import com.adroited.aiterm.security.AppLock

/** Process-scoped dependencies; no pairing secret is ever retained here. */
class AppContainer(context: Context) {
    val pairedDesktopStore = SharedPreferencesPairedDesktopStore(context.applicationContext)
    val deviceKeys = AndroidDeviceKeyStore()
    val pairingRepository = PairingRepository(
        transport = OkHttpPairingTransport(),
        deviceKeys = deviceKeys,
        store = pairedDesktopStore,
    )
    val appLock = AppLock().apply {
        // A process restart must not reveal a previously paired desktop merely
        // because the in-memory background timestamp died with the process.
        val hasPairedOrUnreadableData = try {
            pairedDesktopStore.all().isNotEmpty()
        } catch (_: PairedDesktopStoreException) {
            true
        }
        if (hasPairedOrUnreadableData) lockNow()
    }
}
