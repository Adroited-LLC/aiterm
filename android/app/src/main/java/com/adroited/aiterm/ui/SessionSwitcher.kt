package com.adroited.aiterm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.ExpandLess
import androidx.compose.material.icons.filled.ExpandMore
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.adroited.aiterm.remote.RemoteClientState
import com.adroited.aiterm.remote.RemoteSession
import com.adroited.aiterm.ui.theme.AgentIcon

@Composable
internal fun SessionSwitcher(
    state: RemoteClientState,
    session: RemoteSession,
    enabled: Boolean = true,
    onSelectSession: (RemoteSession) -> Unit,
    subtitle: @Composable () -> Unit,
) {
    var expanded by remember(session.id) { mutableStateOf(false) }
    val choices = state.sessions.filter { isConversationSessionLive(it, state.tabs) }
        .distinctBy { it.id }.sortedByDescending { it.id == session.id }
    Box {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 48.dp)
                .clickable(enabled = enabled, onClickLabel = "Switch session") { expanded = !expanded }
                .padding(vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            AgentIcon(session.agent, size = 26.dp)
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(session.title.ifBlank { "Untitled session" },
                        modifier = Modifier.weight(1f, fill = false),
                        style = MaterialTheme.typography.titleMedium,
                        maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Icon(if (expanded) Icons.Filled.ExpandLess else Icons.Filled.ExpandMore,
                        contentDescription = "Switch session", modifier = Modifier.size(20.dp),
                        tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                subtitle()
            }
        }
        DropdownMenu(
            expanded = expanded && enabled, onDismissRequest = { expanded = false },
            modifier = Modifier.width(300.dp).heightIn(max = 440.dp),
            shape = RoundedCornerShape(20.dp),
            containerColor = MaterialTheme.colorScheme.surfaceContainer,
        ) {
            Text("Live sessions", style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 10.dp))
            if (choices.isEmpty()) {
                Text("No live sessions", style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp))
            }
            choices.forEach { candidate ->
                val current = candidate.id == session.id
                DropdownMenuItem(
                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp)
                        .background(if (current) MaterialTheme.colorScheme.primary.copy(alpha = 0.10f) else Color.Transparent,
                            RoundedCornerShape(12.dp)),
                    text = {
                        Column(Modifier.padding(vertical = 6.dp)) {
                            Text(candidate.title.ifBlank { "Untitled session" },
                                style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            val project = candidate.groupPath.ifBlank { candidate.projectPath }
                                .trimEnd('/').substringAfterLast('/')
                            Text(listOfNotNull(if (current) "Current session" else candidate.agent,
                                project.takeIf { it.isNotBlank() }).joinToString(" · "),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    },
                    leadingIcon = { AgentIcon(candidate.agent, size = 24.dp) },
                    trailingIcon = { if (current) Icon(Icons.Filled.Check, "Selected session",
                        tint = MaterialTheme.colorScheme.primary) },
                    onClick = { expanded = false; if (!current) onSelectSession(candidate) },
                )
            }
        }
    }
}
