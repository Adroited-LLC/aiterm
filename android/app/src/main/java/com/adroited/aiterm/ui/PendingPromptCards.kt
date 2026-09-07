package com.adroited.aiterm.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.adroited.aiterm.remote.PendingConversationPrompt

/** These are local submissions awaiting transcript confirmation, never synthetic transcript rows. */
@Composable
internal fun PendingPromptCards(
    prompts: List<PendingConversationPrompt>,
    connected: Boolean,
    onHide: (String) -> Unit,
) {
    if (prompts.isEmpty()) return
    Column(Modifier.fillMaxWidth().heightIn(max = 168.dp).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(6.dp)) {
        prompts.forEach { prompt ->
            Surface(shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.secondaryContainer) {
                Column(Modifier.fillMaxWidth().padding(start = 12.dp, end = 8.dp, bottom = 12.dp)) {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Column(Modifier.weight(1f).padding(top = 10.dp)) {
                            Text("Pending prompt", style = MaterialTheme.typography.labelLarge)
                            Text(when {
                                !prompt.accepted -> "Sending to desktop…"
                                !connected -> "Sent · waiting for connection to confirm"
                                else -> "Sent · waiting for agent"
                            }, style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSecondaryContainer)
                        }
                        if (prompt.accepted) TextButton(onClick = { onHide(prompt.id) }) { Text("Hide") }
                    }
                    Text(prompt.text, style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
    }
    Spacer(Modifier.height(6.dp))
}
