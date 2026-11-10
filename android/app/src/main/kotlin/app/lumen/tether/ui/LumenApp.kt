package app.lumen.tether.ui

import android.content.res.Configuration
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.windowsizeclass.WindowWidthSizeClass
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import app.lumen.tether.LumenViewModel
import uniffi.lumen_android.CameraSetting

@Composable
fun LumenTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = darkColorScheme()) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background, content = content)
    }
}

/** Phones and any portrait screen stack the controls under the picture; landscape tablets put them in a side panel. */
@Composable
fun LumenApp(vm: LumenViewModel, width: WindowWidthSizeClass) {
    Box(Modifier.fillMaxSize().safeDrawingPadding()) {
        if (!vm.connected) {
            ConnectScreen(vm)
        } else if (width == WindowWidthSizeClass.Compact || LocalConfiguration.current.orientation == Configuration.ORIENTATION_PORTRAIT) {
            Column(Modifier.fillMaxSize()) {
                LiveView(vm, Modifier.fillMaxWidth().weight(1f))
                Controls(vm, Modifier.fillMaxWidth().weight(1f))
            }
        } else {
            Row(Modifier.fillMaxSize()) {
                LiveView(vm, Modifier.weight(1f).fillMaxHeight())
                Controls(vm, Modifier.width(if (width == WindowWidthSizeClass.Medium) 320.dp else 400.dp).fillMaxHeight())
            }
        }
        vm.message?.let { msg ->
            Surface(
                onClick = vm::dismissMessage,
                modifier = Modifier.align(Alignment.BottomCenter).padding(16.dp),
                color = MaterialTheme.colorScheme.inverseSurface,
                shape = MaterialTheme.shapes.medium,
            ) { Text(msg, Modifier.padding(horizontal = 16.dp, vertical = 12.dp), color = MaterialTheme.colorScheme.inverseOnSurface) }
        }
    }
}

@Composable
private fun ConnectScreen(vm: LumenViewModel) {
    val cameras = remember { mutableStateOf(vm.usb.cameras()) }
    Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        Text("Lumen Tether Studio", style = MaterialTheme.typography.headlineMedium)
        Text("Connect a camera with a USB cable (OTG adapter on phones).", Modifier.padding(vertical = 12.dp))
        cameras.value.forEach { device ->
            Button(onClick = { vm.usb.request(device) }, Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
                Text(device.productName ?: "USB camera %04x:%04x".format(device.vendorId, device.productId))
            }
        }
        OutlinedButton(onClick = { cameras.value = vm.usb.cameras() }, Modifier.padding(top = 8.dp)) { Text("Rescan USB") }
        OutlinedButton(onClick = vm::connectSimulator, Modifier.padding(top = 8.dp)) { Text("Virtual DSLR (simulator)") }
    }
}

@Composable
private fun LiveView(vm: LumenViewModel, modifier: Modifier) {
    Box(modifier.background(Color.Black), contentAlignment = Alignment.Center) {
        val frame = vm.frame
        if (frame != null) {
            Image(frame.asImageBitmap(), contentDescription = "Live view", Modifier.fillMaxSize(), contentScale = ContentScale.Fit)
        } else {
            Text(if (vm.liveView) "Waiting for live view..." else "Live view off", color = Color.Gray)
        }
    }
}

@Composable
private fun Controls(vm: LumenViewModel, modifier: Modifier) {
    Column(modifier.verticalScroll(rememberScrollState()).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        vm.camera?.let { Text("${it.manufacturer} ${it.model}", style = MaterialTheme.typography.titleMedium) }
        val caps = vm.caps
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = vm::capture, Modifier.weight(1f)) { Text("Shoot") }
            if (caps?.record == true) {
                OutlinedButton(onClick = { vm.record(!vm.recording) }, Modifier.weight(1f)) { Text(if (vm.recording) "Stop" else "Record") }
            }
        }
        if (caps?.focusDrive == true) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { vm.focus(far = false) }, Modifier.weight(1f)) { Text("Near") }
                if (caps.autofocus) OutlinedButton(onClick = vm::autofocus, Modifier.weight(1f)) { Text("AF") }
                OutlinedButton(onClick = { vm.focus(far = true) }, Modifier.weight(1f)) { Text("Far") }
            }
        }
        vm.settings.filter { it.writable && it.choices.isNotEmpty() }.forEach { SettingPicker(vm, it) }
        OutlinedButton(onClick = vm::disconnect, Modifier.fillMaxWidth()) { Text("Disconnect") }
    }
}

@Composable
private fun SettingPicker(vm: LumenViewModel, setting: CameraSetting) {
    var open by remember { mutableStateOf(false) }
    Box {
        OutlinedButton(onClick = { open = true }, Modifier.fillMaxWidth()) { Text("${setting.label}: ${setting.text}") }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            setting.choices.forEach { choice ->
                DropdownMenuItem(text = { Text(choice.text) }, onClick = { open = false; vm.set(setting, choice.value) })
            }
        }
    }
}
