package com.fivelime.aiterm.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import com.fivelime.aiterm.AppViewModel

/** Three screens and no navigation library: not paired → pair; paired →
 *  the list; a session picked → that session. Back goes up one. */
@Composable
fun AitermApp(vm: AppViewModel) {
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(vm.notice) {
        val n = vm.notice ?: return@LaunchedEffect
        vm.notice = null
        snackbar.showSnackbar(n)
    }
    BackHandler(enabled = vm.previewUrl != null) { vm.previewUrl = null }
    BackHandler(enabled = vm.showSettings) { vm.showSettings = false }
    BackHandler(enabled = vm.viewing != null) { vm.viewing = null }
    BackHandler(enabled = vm.viewing == null && vm.selected != null && vm.showFiles) {
        if (!(vm.browsing && vm.browseUp())) vm.showFiles = false
    }
    BackHandler(enabled = vm.composingNew) { vm.composingNew = false }
    BackHandler(enabled = vm.selected != null && !vm.composingNew && !vm.showFiles && vm.viewing == null) { vm.select(null) }

    Scaffold(
        modifier = Modifier.fillMaxSize(),
        snackbarHost = { SnackbarHost(snackbar) },
        // The screens' own top bars handle the status bar; padding here too
        // would apply it twice.
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
    ) { padding ->
        when {
            vm.locked -> LockScreen(vm, padding)
            vm.desktop == null -> PairScreen(vm, padding)
            vm.previewUrl != null -> PreviewScreen(vm, vm.previewUrl!!, padding)
            vm.showSettings -> SettingsScreen(vm, padding)
            vm.composingNew -> NewSessionScreen(vm, padding)
            vm.viewing != null -> FileViewer(vm, vm.viewing!!.first, vm.viewing!!.second, padding)
            vm.selected == null -> SessionsScreen(vm, padding)
            else -> SessionScreen(vm, vm.selected!!, padding)
        }
    }
}
