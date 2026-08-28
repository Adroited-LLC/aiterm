package com.adroited.aiterm

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import com.adroited.aiterm.ui.AitermApp

/**
 * The only Activity in the app. Every screen is a Compose destination inside
 * [AitermApp]; there is no WebView anywhere in this client.
 */
class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent { AitermApp() }
    }
}
