package com.adroited.aiterm.ui

import androidx.compose.runtime.Composable
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import com.adroited.aiterm.ui.theme.AitermTheme
import kotlinx.serialization.Serializable

/** Type-safe navigation destinations. */
@Serializable
object DesktopsRoute

@Serializable
object PairingRoute

/**
 * The navigation shell. The start destination is always the paired-desktop
 * list; pairing is reached from it, never the other way round, so a returning
 * user with a paired desktop never sees the camera.
 */
@Composable
fun AitermApp(navController: NavHostController = rememberNavController()) {
    AitermTheme {
        NavHost(navController = navController, startDestination = DesktopsRoute) {
            composable<DesktopsRoute> {
                DesktopListScreen(onPairDesktop = { navController.navigate(PairingRoute) })
            }
            composable<PairingRoute> {
                // Task 8 replaces this with ui/PairingScreen.kt (camera preview,
                // fingerprint confirmation, keystore enrollment).
                PairingPlaceholderScreen(onBack = { navController.popBackStack() })
            }
        }
    }
}
