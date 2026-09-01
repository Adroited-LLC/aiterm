package com.adroited.aiterm.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import com.adroited.aiterm.ui.theme.AitermTheme
import com.adroited.aiterm.AitermApplication
import com.adroited.aiterm.AppContainer
import kotlinx.serialization.Serializable
import androidx.navigation.toRoute

/** Type-safe navigation destinations. */
@Serializable
object DesktopsRoute

@Serializable
object PairingRoute

@Serializable
data class TerminalRoute(val deviceId: String)

/**
 * The navigation shell. The start destination is always the paired-desktop
 * list; pairing is reached from it, never the other way round, so a returning
 * user with a paired desktop never sees the camera.
 */
@Composable
fun AitermApp(
    navController: NavHostController = rememberNavController(),
    onRequestUnlock: () -> Unit = {},
    unlockError: String? = null,
    dependencies: AppContainer? = null,
) {
    val application = LocalContext.current.applicationContext as AitermApplication
    val container = dependencies ?: application.container
    val locked by container.appLock.isLocked.collectAsStateWithLifecycle()

    AitermTheme {
        if (locked) {
            LockedContent(onUnlock = onRequestUnlock, error = unlockError)
        } else {
            NavHost(navController = navController, startDestination = DesktopsRoute) {
                composable<DesktopsRoute> {
                    DesktopListScreen(
                        store = container.pairedDesktopStore,
                        onPairDesktop = { navController.navigate(PairingRoute) },
                        onOpenDesktop = { navController.navigate(TerminalRoute(it.deviceId)) },
                    )
                }
                composable<PairingRoute> {
                    PairingScreen(
                        repository = container.pairingRepository,
                        onBack = { navController.popBackStack() },
                        onPaired = { navController.popBackStack() },
                    )
                }
                composable<TerminalRoute> { entry ->
                    val route = entry.toRoute<TerminalRoute>()
                    val desktop = runCatching { container.pairedDesktopStore.all() }
                        .getOrDefault(emptyList())
                        .firstOrNull { it.deviceId == route.deviceId }
                    if (desktop == null) {
                        LaunchedEffect(route.deviceId) { navController.popBackStack() }
                    } else {
                        val remoteViewModel: RemoteTerminalViewModel = viewModel(
                            key = "remote-${desktop.deviceId}",
                            factory = RemoteTerminalViewModel.factory(
                                desktop,
                                container.deviceKeys,
                                container.appLock,
                                container.pairedDesktopStore,
                            ),
                        )
                        RemoteDesktopScreen(
                            viewModel = remoteViewModel,
                            desktopName = desktop.displayName,
                            onBack = { navController.popBackStack() },
                            keyBarPreference = container.terminalKeyBarPreference,
                        )
                    }
                }
            }
        }
    }
}
