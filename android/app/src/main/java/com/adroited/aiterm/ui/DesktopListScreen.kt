package com.adroited.aiterm.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Computer
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.adroited.aiterm.R
import com.adroited.aiterm.pairing.PairedDesktop
import com.adroited.aiterm.pairing.PairedDesktopStore

/** The desktops trusted by this phone, with pairing and removal in one place. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DesktopListScreen(
    store: PairedDesktopStore,
    onPairDesktop: () -> Unit,
    onOpenDesktop: (PairedDesktop) -> Unit = {},
    onBack: (() -> Unit)? = null,
    viewModel: DesktopListViewModel = viewModel(factory = DesktopListViewModel.factory(store)),
) {
    val uiState by viewModel.uiState.collectAsStateWithLifecycle()
    var forgetTarget by remember { mutableStateOf<PairedDesktop?>(null) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { viewModel.refresh() }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.desktops_title)) },
                navigationIcon = {
                    if (onBack != null) {
                        IconButton(onClick = onBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back to main screen")
                        }
                    }
                },
            )
        },
    ) { innerPadding ->
        PullToRefreshBox(
            isRefreshing = false,
            onRefresh = viewModel::refresh,
            modifier = Modifier.fillMaxSize().padding(innerPadding),
        ) {
            LazyColumn(
                modifier = Modifier.fillMaxSize().testTag("desktop-list"),
                contentPadding = PaddingValues(horizontal = 20.dp, vertical = 16.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                if (uiState.storageFailure) {
                    item {
                        Column(
                            modifier = Modifier.widthIn(max = 640.dp).fillMaxWidth().padding(vertical = 32.dp),
                            verticalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            Text("Unable to load desktops", style = MaterialTheme.typography.headlineSmall)
                            Text(
                                "Paired desktop storage could not be read. Your saved data is unchanged. Pull down to try again.",
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                } else if (uiState.desktops.isEmpty()) {
                    item {
                        EmptyDesktopList(
                            onPairDesktop = onPairDesktop,
                            modifier = Modifier.widthIn(max = 640.dp).fillMaxWidth().padding(vertical = 48.dp),
                        )
                    }
                } else {
                    item {
                        Column(
                            modifier = Modifier.widthIn(max = 640.dp).fillMaxWidth(),
                            verticalArrangement = Arrangement.spacedBy(16.dp),
                        ) {
                            Text(
                                "Connect to your desktop",
                                style = MaterialTheme.typography.headlineSmall,
                            )
                            Text(
                                "Connect to a paired desktop or add another.",
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Button(onClick = onPairDesktop) {
                                Icon(Icons.Default.Add, contentDescription = null, modifier = Modifier.size(18.dp))
                                Text(stringResource(R.string.action_pair_desktop), modifier = Modifier.padding(start = 8.dp))
                            }
                        }
                    }
                    items(uiState.desktops, key = PairedDesktop::deviceId) { desktop ->
                        DesktopCard(
                            desktop = desktop,
                            onOpen = { onOpenDesktop(desktop) },
                            onForget = { forgetTarget = desktop },
                            modifier = Modifier.widthIn(max = 640.dp).fillMaxWidth(),
                        )
                    }
                }
            }
        }
    }

    forgetTarget?.let { desktop ->
        AlertDialog(
            onDismissRequest = { forgetTarget = null },
            title = {
                Text(stringResource(R.string.forget_desktop_title, desktop.displayName))
            },
            text = {
                Text(stringResource(R.string.forget_desktop_body))
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        viewModel.forget(desktop.deviceId)
                        forgetTarget = null
                    },
                    colors = ButtonDefaults.textButtonColors(
                        contentColor = MaterialTheme.colorScheme.error,
                    ),
                ) { Text(stringResource(R.string.action_confirm_forget_desktop)) }
            },
            dismissButton = {
                TextButton(onClick = { forgetTarget = null }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }
}

@Composable
private fun DesktopCard(
    desktop: PairedDesktop,
    onOpen: () -> Unit,
    onForget: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var showIdentity by rememberSaveable(desktop.deviceId) { mutableStateOf(false) }
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(20.dp),
        color = MaterialTheme.colorScheme.surface,
        border = BorderStroke(
            1.dp,
            MaterialTheme.colorScheme.outlineVariant,
        ),
    ) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp), verticalAlignment = Alignment.CenterVertically) {
                Surface(
                    shape = RoundedCornerShape(12.dp),
                    color = MaterialTheme.colorScheme.surfaceContainerHighest,
                ) {
                    Box(modifier = Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                        Icon(Icons.Default.Computer, contentDescription = null, tint = MaterialTheme.colorScheme.primary)
                    }
                }
                Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(desktop.displayName, style = MaterialTheme.typography.titleLarge)
                    Text(
                        "Paired desktop",
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            desktop.hosts.firstOrNull()?.let { host ->
                Text(host, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                OutlinedButton(onClick = onOpen) { Text("Connect") }
                TextButton(
                    onClick = onForget,
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                ) { Text(stringResource(R.string.action_forget_desktop)) }
            }
            TextButton(onClick = { showIdentity = !showIdentity }) {
                Text(if (showIdentity) "Hide identity" else "View identity")
            }
            if (showIdentity) {
                Text(
                    "Desktop fingerprint",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    desktop.serverSpkiFingerprint.chunked(4).joinToString("-"),
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                )
            }
        }
    }
}

@Composable
private fun EmptyDesktopList(onPairDesktop: () -> Unit, modifier: Modifier = Modifier) {
    Column(
        modifier = modifier,
        verticalArrangement = Arrangement.spacedBy(20.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(
            Icons.Default.Computer,
            contentDescription = null,
            modifier = Modifier.size(64.dp),
            tint = MaterialTheme.colorScheme.primary,
        )
        Text(text = stringResource(R.string.desktops_empty_title), style = MaterialTheme.typography.headlineSmall)
        Text(
            text = stringResource(R.string.desktops_empty_body),
            textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodyLarge,
        )
        Button(onClick = onPairDesktop) {
            Icon(Icons.Default.Add, contentDescription = null, modifier = Modifier.size(18.dp))
            Text(stringResource(R.string.action_pair_desktop), modifier = Modifier.padding(start = 8.dp))
        }
    }
}
