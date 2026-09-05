package com.adroited.aiterm.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.pairing.PairedDesktopStore
import com.adroited.aiterm.pairing.PairedDesktopStoreException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** What the desktop list renders. */
data class DesktopListUiState(
    val desktops: List<PairedDesktop> = emptyList(),
    val storageFailure: Boolean = false,
)

/**
 * Holds the paired-desktop list. Task 8 injects the pairing repository here and
 * replaces the empty seed with the persisted records.
 */
class DesktopListViewModel(private val store: PairedDesktopStore) : ViewModel() {

    private val _uiState = MutableStateFlow(DesktopListUiState())
    val uiState: StateFlow<DesktopListUiState> = _uiState.asStateFlow()

    init {
        refresh()
    }

    fun refresh() {
        _uiState.value = try {
            DesktopListUiState(desktops = store.all())
        } catch (_: PairedDesktopStoreException) {
            DesktopListUiState(storageFailure = true)
        }
    }

    fun forget(deviceId: String) {
        try {
            store.remove(deviceId)
            refresh()
        } catch (_: PairedDesktopStoreException) {
            _uiState.value = _uiState.value.copy(storageFailure = true)
        }
    }

    fun saveFriendlyName(deviceId: String, name: String): Boolean {
        val trimmed = name.trim()
        if (trimmed.length > 128 || name.any(Char::isISOControl)) return false
        return try {
            store.updateExisting(deviceId) { it.copy(friendlyName = trimmed.ifEmpty { null }) }
                ?: return false
            _uiState.value = DesktopListUiState(desktops = store.all())
            true
        } catch (_: PairedDesktopStoreException) {
            false
        }
    }

    companion object {
        fun factory(store: PairedDesktopStore): ViewModelProvider.Factory = viewModelFactory {
            initializer { DesktopListViewModel(store) }
        }
    }
}
