package com.fivelime.aiterm.ui

import android.annotation.SuppressLint
import android.net.http.SslError
import android.webkit.SslErrorHandler
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.viewinterop.AndroidView
import com.fivelime.aiterm.AppViewModel
import java.security.MessageDigest

/** A page the agent built, live: the desktop serves it (a static folder or
 *  a proxied dev server) behind a ticketed path, and this WebView trusts
 *  exactly one certificate — the desktop's own, by pinned fingerprint. */
@OptIn(ExperimentalMaterial3Api::class)
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun PreviewScreen(vm: AppViewModel, url: String, outer: PaddingValues) {
    val fingerprint = vm.desktop?.fingerprint ?: ""
    val web = remember { java.util.concurrent.atomic.AtomicReference<WebView?>(null) }
    Scaffold(
        modifier = Modifier.padding(outer),
        containerColor = Bg,
        topBar = {
            TopAppBar(
                navigationIcon = { IconButton(onClick = { vm.previewUrl = null }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
                title = { Text("Preview", maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMedium) },
                actions = { IconButton(onClick = { web.get()?.reload() }) { Icon(Icons.Filled.Refresh, "Reload") } },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Bg),
            )
        },
    ) { padding ->
        AndroidView(
            modifier = Modifier.fillMaxSize().padding(padding),
            factory = { ctx ->
                WebView(ctx).apply {
                    settings.javaScriptEnabled = true
                    settings.domStorageEnabled = true
                    settings.loadWithOverviewMode = true
                    settings.useWideViewPort = true
                    settings.builtInZoomControls = true
                    settings.displayZoomControls = false
                    webViewClient = PinnedClient(fingerprint)
                    web.set(this)
                    loadUrl(url)
                }
            },
        )
    }
}

/** Trust the desktop's self-signed certificate and nothing else. */
private class PinnedClient(private val fingerprint: String) : WebViewClient() {
    override fun onReceivedSslError(view: WebView?, handler: SslErrorHandler, error: SslError) {
        val cert = if (android.os.Build.VERSION.SDK_INT >= 29) error.certificate.x509Certificate else null
        val hex = cert?.let {
            MessageDigest.getInstance("SHA-256").digest(it.encoded).joinToString("") { b -> "%02x".format(b) }
        }
        if (hex != null && hex.equals(fingerprint, ignoreCase = true)) handler.proceed() else handler.cancel()
    }
}
