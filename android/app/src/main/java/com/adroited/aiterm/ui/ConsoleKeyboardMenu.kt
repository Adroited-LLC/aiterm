package com.adroited.aiterm.ui

import android.content.ComponentName
import android.content.Intent
import android.provider.Settings
import android.view.inputmethod.InputMethodManager
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import com.adroited.aiterm.keyboard.ConsoleKeyboardService

@Composable
internal fun ConsoleKeyboardMenu() {
    val context = LocalContext.current
    var open by remember { mutableStateOf(false) }
    var setup by remember { mutableStateOf(false) }
    IconButton(onClick = { open = true }) {
        Icon(Icons.Filled.MoreVert, contentDescription = "Terminal options")
    }
    DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
        DropdownMenuItem(text = { Text("Console keyboard") }, onClick = {
            open = false
            val manager = context.getSystemService(InputMethodManager::class.java)
            val console = ComponentName(context, ConsoleKeyboardService::class.java)
            if (manager.enabledInputMethodList.any { ComponentName.unflattenFromString(it.id) == console }) {
                manager.showInputMethodPicker()
            } else {
                setup = true
            }
        })
    }
    if (setup) {
        AlertDialog(
            onDismissRequest = { setup = false },
            title = { Text("Set up console keyboard") },
            text = { Text("Enable aiterm Console in Android’s keyboard settings, then use the globe to switch between your normal keyboard and direct console input.") },
            confirmButton = {
                TextButton(onClick = {
                    setup = false
                    context.startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
                }) { Text("Open keyboard settings") }
            },
            dismissButton = { TextButton(onClick = { setup = false }) { Text("Cancel") } },
        )
    }
}
