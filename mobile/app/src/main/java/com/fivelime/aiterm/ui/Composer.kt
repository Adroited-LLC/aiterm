package com.fivelime.aiterm.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.fivelime.aiterm.AppViewModel
import com.fivelime.aiterm.Attachment

/** The `+` in a composer: pick any file or image; it uploads to the desktop
 *  and appears as a chip until sent. */
@Composable
fun AttachButton(vm: AppViewModel) {
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri -> uri?.let(vm::attach) }
    if (vm.uploading) {
        CircularProgressIndicator(Modifier.padding(12.dp).size(22.dp), strokeWidth = 2.dp)
    } else {
        IconButton(onClick = { pick.launch("*/*") }) { Icon(Icons.Filled.Add, "Attach a file or image", tint = Muted) }
    }
}

@Composable
fun AttachmentChips(vm: AppViewModel) {
    if (vm.attachments.isEmpty()) return
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 4.dp)) {
        vm.attachments.forEach { a ->
            Row(
                Modifier.padding(end = 6.dp).background(Surface2, RoundedCornerShape(12.dp)).padding(start = 10.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.Filled.AttachFile, null, tint = Accent, modifier = Modifier.size(14.dp))
                Spacer(Modifier.width(4.dp))
                Text(a.name, style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.width(140.dp))
                IconButton(onClick = { vm.removeAttachment(a) }, modifier = Modifier.size(24.dp)) { Icon(Icons.Filled.Close, "Remove", tint = Muted, modifier = Modifier.size(14.dp)) }
            }
        }
    }
}

/** A pill that opens a menu — the model / effort / harness pickers. */
@Composable
fun PickerChip(label: String, options: List<Pair<String, String>>, onPick: (String) -> Unit, leading: (@Composable () -> Unit)? = null) {
    var open by remember { mutableStateOf(false) }
    Row(
        Modifier.background(Surface2, RoundedCornerShape(16.dp)).clickable { open = true }.padding(horizontal = 10.dp, vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        leading?.let { it(); Spacer(Modifier.width(6.dp)) }
        Text(label, style = MaterialTheme.typography.labelLarge, color = Color(0xFFE6EAF2))
        Icon(Icons.Filled.KeyboardArrowDown, null, tint = Muted, modifier = Modifier.size(16.dp))
    }
    DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
        options.forEach { (id, name) -> DropdownMenuItem(text = { Text(name) }, onClick = { open = false; onPick(id) }) }
    }
}
