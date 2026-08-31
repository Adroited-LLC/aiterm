package com.fivelime.aiterm.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.fivelime.aiterm.AppViewModel

private val THEMES = listOf("dark" to "Deep blue", "black" to "Black", "nord" to "Nord", "light" to "Light")

/** The short list a person actually picks from; anything else can be added
 *  when someone needs it. Empty id = the phone's own zone. */
private val ZONES = listOf(
    "" to "Phone's time zone",
    "America/New_York" to "Eastern (New York)",
    "America/Chicago" to "Central (Chicago)",
    "America/Denver" to "Mountain (Denver)",
    "America/Los_Angeles" to "Pacific (Los Angeles)",
    "UTC" to "UTC",
)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(vm: AppViewModel, outer: PaddingValues) {
    Scaffold(
        modifier = Modifier.padding(outer),
        containerColor = Bg,
        topBar = {
            TopAppBar(
                navigationIcon = { IconButton(onClick = { vm.showSettings = false }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
                title = { Text("Settings") },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = Bg),
            )
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState())) {
            Text("THEME", style = MaterialTheme.typography.labelSmall, color = Muted,
                modifier = Modifier.padding(start = 20.dp, top = 8.dp, bottom = 4.dp))
            THEMES.forEach { (id, label) ->
                OptionRow(label, selected = vm.themeName == id) { vm.setTheme(id) }
            }
            HorizontalDivider(Modifier.padding(vertical = 12.dp), color = Surface1)
            Text("TIME ZONE", style = MaterialTheme.typography.labelSmall, color = Muted,
                modifier = Modifier.padding(start = 20.dp, bottom = 4.dp))
            Text(
                "How dates and reset times are written. Useful when the desktop lives in another zone.",
                style = MaterialTheme.typography.bodySmall, color = Muted,
                modifier = Modifier.padding(horizontal = 20.dp),
            )
            ZONES.forEach { (id, label) ->
                OptionRow(label, selected = vm.timeZone == id) { vm.setTz(id) }
            }
            HorizontalDivider(Modifier.padding(vertical = 12.dp), color = Surface1)
            Text("SECURITY", style = MaterialTheme.typography.labelSmall, color = Muted,
                modifier = Modifier.padding(start = 20.dp, bottom = 4.dp))
            Row(
                Modifier.fillMaxWidth().clickable { vm.setBiometricEnabled(!vm.biometric) }
                    .padding(horizontal = 20.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text("Require fingerprint")
                    Text("Lock the app on open and after 5 minutes away",
                        style = MaterialTheme.typography.bodySmall, color = Muted)
                }
                Switch(checked = vm.biometric, onCheckedChange = { vm.setBiometricEnabled(it) })
            }
        }
    }
}

@Composable
private fun OptionRow(label: String, selected: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = selected, onClick = onClick)
        Spacer(Modifier.width(4.dp))
        Text(label)
    }
}
