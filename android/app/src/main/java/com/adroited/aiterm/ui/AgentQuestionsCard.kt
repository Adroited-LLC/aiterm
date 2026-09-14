package com.adroited.aiterm.ui

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.adroited.aiterm.remote.Item

internal val LocalQuestionAction = staticCompositionLocalOf<(() -> Unit)?> { null }

@Composable
internal fun AgentQuestionsCard(item: Item.Tool) {
    val questions = remember(item.input, item.tool) { agentQuestions(item).orEmpty() }
    val answer = LocalQuestionAction.current
    OutlinedCard(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(if (questions.isEmpty()) "Questions" else "Questions · ${questions.size}",
                style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.primary)
            if (questions.isEmpty()) {
                Text("This desktop supplied a shortened question. Open the terminal to read and answer it.",
                    style = MaterialTheme.typography.bodyMedium)
            }
            questions.forEachIndexed { index, question ->
                if (index > 0) HorizontalDivider()
                Text(question.title, style = MaterialTheme.typography.bodyLarge)
                if (question.multiple) Text("Multiple choices allowed", style = MaterialTheme.typography.labelMedium)
                question.options.forEach { option ->
                    Column(Modifier.padding(start = 12.dp)) {
                        Text("• ${option.label}", style = MaterialTheme.typography.bodyMedium)
                        if (option.description.isNotBlank()) Text(option.description,
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
            if (answer != null) {
                Text("Answer any questions still open using the agent’s live controls.",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                OutlinedButton(onClick = answer) { Text("Answer in terminal") }
            }
        }
    }
}

internal fun questionNavigationKeys(applicationCursor: Boolean): List<Pair<String, String>> = listOf(
    "↑" to if (applicationCursor) "\u001bOA" else "\u001b[A",
    "↓" to if (applicationCursor) "\u001bOB" else "\u001b[B",
    "Tab" to "\t", "Space" to " ", "Enter" to "\r", "Esc" to "\u001b",
)

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun QuestionTerminalControls(
    enabled: Boolean,
    codex: Boolean,
    applicationCursor: Boolean,
    onKey: (String) -> Unit,
    onClose: () -> Unit,
) {
    Column {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            if (codex) TextButton(enabled = enabled, onClick = { onKey("\u001b[1;3A") }) {
                Text("Open queued questions")
            } else Text("Question controls", style = MaterialTheme.typography.labelLarge)
            TextButton(onClick = onClose) { Text("Hide") }
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            questionNavigationKeys(applicationCursor).forEach { (label, value) ->
                OutlinedButton(enabled = enabled, onClick = { onKey(value) },
                    contentPadding = PaddingValues(horizontal = 12.dp, vertical = 4.dp)) { Text(label) }
            }
        }
    }
}
