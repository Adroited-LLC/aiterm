package com.adroited.aiterm.ui

import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.pairing.PairedDesktopStore
import com.adroited.aiterm.pairing.PairedDesktopStoreException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DesktopListViewModelTest {

    @Test
    fun renameTrimsAndClearsTheAliasWhileKeepingTheLatestConnectionMetadata() {
        val original = desktop()
        val store = MemoryDesktopStore(listOf(original))
        val viewModel = DesktopListViewModel(store)
        store.save(original.copy(hosts = listOf("10.0.0.152")))

        assertTrue(viewModel.saveFriendlyName(original.deviceId, "  Home office  "))
        assertEquals("Home office", store.all().single().label)
        assertEquals(listOf("10.0.0.152"), store.all().single().hosts)
        assertEquals("Home office", viewModel.uiState.value.desktops.single().label)
        assertTrue(viewModel.saveFriendlyName(original.deviceId, "   "))
        assertEquals(null, store.all().single().friendlyName)
        assertEquals(original.displayName, store.all().single().label)
    }

    @Test
    fun invalidNamesAndMissingDesktopsAreNotSaved() {
        val original = desktop()
        val store = MemoryDesktopStore(listOf(original))
        val viewModel = DesktopListViewModel(store)
        for (invalid in listOf("x".repeat(129), "office\nPC", "\tOffice")) {
            assertFalse(viewModel.saveFriendlyName(original.deviceId, invalid))
            assertEquals(listOf(original), store.all())
        }
        assertTrue(viewModel.saveFriendlyName(original.deviceId, "x".repeat(128)))
        store.remove(original.deviceId)
        assertFalse(viewModel.saveFriendlyName(original.deviceId, "Forgotten"))
        assertTrue(store.all().isEmpty())
    }

    @Test
    fun failedRenameReturnsFalseAndKeepsTheDisplayedRecord() {
        val original = desktop()
        val store = object : PairedDesktopStore {
            override fun all() = listOf(original)
            override fun save(desktop: PairedDesktop) {
                throw PairedDesktopStoreException("Could not save desktop")
            }
            override fun remove(deviceId: String) = Unit
        }
        val viewModel = DesktopListViewModel(store)
        assertFalse(viewModel.saveFriendlyName(original.deviceId, "Office"))
        assertEquals(listOf(original), viewModel.uiState.value.desktops)
    }

    @Test
    fun routeRefreshUsesTheLatestAliasAndCannotRecreateAForgottenDesktop() {
        val original = desktop()
        val store = MemoryDesktopStore(listOf(original))
        store.updateExisting(original.deviceId) { it.copy(friendlyName = "Office") }
        val updated = store.updateExisting(original.deviceId) { it.copy(hosts = listOf("10.0.0.153")) }
        assertEquals("Office", updated?.label)
        store.remove(original.deviceId)
        assertEquals(null, store.updateExisting(original.deviceId) { it.copy(port = 9000) })
        assertTrue(store.all().isEmpty())
    }

    @Test
    fun forgetRemovesTheDesktopFromThePublishedList() {
        val store = MemoryDesktopStore(listOf(desktop()))
        val viewModel = DesktopListViewModel(store)

        viewModel.forget("desktop-1")

        assertEquals(emptyList<PairedDesktop>(), viewModel.uiState.value.desktops)
        assertEquals(emptyList<PairedDesktop>(), store.all())
    }

    @Test
    fun failedForgetPreservesThePublishedDesktopAndReportsStorageFailure() {
        val storedDesktop = desktop()
        val store = object : PairedDesktopStore {
            override fun all(): List<PairedDesktop> = listOf(storedDesktop)

            override fun save(desktop: PairedDesktop) = Unit

            override fun remove(deviceId: String) {
                throw PairedDesktopStoreException("Could not remove desktop")
            }
        }
        val viewModel = DesktopListViewModel(store)

        viewModel.forget(storedDesktop.deviceId)

        assertEquals(listOf(storedDesktop), viewModel.uiState.value.desktops)
        assertTrue(viewModel.uiState.value.storageFailure)
    }

    private fun desktop() = PairedDesktop(
        deviceId = "desktop-1",
        displayName = "Workshop PC",
        hosts = listOf("10.0.0.151"),
        port = 8443,
        serverSpkiFingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        lastSeenEpochMillis = null,
    )

    private class MemoryDesktopStore(seed: List<PairedDesktop>) : PairedDesktopStore {
        private val desktops = seed.toMutableList()

        override fun all(): List<PairedDesktop> = desktops.toList()

        override fun save(desktop: PairedDesktop) {
            desktops.removeAll { it.deviceId == desktop.deviceId }
            desktops += desktop
        }

        override fun remove(deviceId: String) {
            desktops.removeAll { it.deviceId == deviceId }
        }
    }
}
