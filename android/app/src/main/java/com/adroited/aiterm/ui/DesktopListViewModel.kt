package com.adroited.aiterm.ui

import androidx.lifecycle.ViewModel
import com.adroited.aiterm.pairing.PairedDesktop
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** What the desktop list renders. */
data class DesktopListUiState(
    val desktops: List<PairedDesktop> = emptyList(),
)

/**
 * Holds the paired-desktop list. Task 8 injects the pairing repository here and
 * replaces the empty seed with the persisted records.
 */
class DesktopListViewModel : ViewModel() {

    private val _uiState = MutableStateFlow(DesktopListUiState())
    val uiState: StateFlow<DesktopListUiState> = _uiState.asStateFlow()
}
