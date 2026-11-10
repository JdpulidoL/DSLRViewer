package app.lumen.tether

import android.app.Application
import android.graphics.Bitmap
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import java.nio.ByteBuffer
import uniffi.lumen_android.CameraCaps
import uniffi.lumen_android.CameraInfo
import uniffi.lumen_android.CameraSetting
import uniffi.lumen_android.DeviceInfo
import uniffi.lumen_android.LiveFrame
import uniffi.lumen_android.LogLevel
import uniffi.lumen_android.LumenListener
import uniffi.lumen_android.LumenSession

/**
 * Bridges the Rust engine to Compose. The listener runs on the engine's event thread, so every state
 * write goes through [post] (Compose snapshot state is thread-safe for writes, but ordering stays simple this way).
 */
class LumenViewModel(app: Application) : AndroidViewModel(app), LumenListener {
    var camera by mutableStateOf<CameraInfo?>(null); private set
    var caps by mutableStateOf<CameraCaps?>(null); private set
    var settings by mutableStateOf<List<CameraSetting>>(emptyList()); private set
    var frame by mutableStateOf<Bitmap?>(null); private set
    var liveView by mutableStateOf(false); private set
    var recording by mutableStateOf(false); private set
    var message by mutableStateOf<String?>(null); private set
    var log by mutableStateOf<List<String>>(emptyList()); private set

    private val session: LumenSession = LumenSession(app.filesDir.absolutePath, this)
    val usb = UsbCamera(app, onOpened = { io, name -> session.connectUsb(io, name) }, onError = { message = it })

    // Registered on the application context so the permission reply survives rotation.
    init { usb.register() }

    val connected: Boolean get() = camera != null

    fun connectSimulator() = session.connectSimulator()
    fun disconnect() {
        session.disconnect()
        usb.close()
    }
    fun requestLiveView(on: Boolean) = session.setLiveView(on)
    fun set(setting: CameraSetting, value: Long) = session.setSetting(setting.prop, value)
    fun focus(far: Boolean, steps: UInt = 1u) = session.focus(far, steps)
    fun autofocus() = session.autofocus()
    fun record(on: Boolean) = session.record(on)
    /** Fires the shutter: only wired to an explicit tap. */
    fun capture() = session.capture()
    fun dismissMessage() { message = null }

    // ---- LumenListener (engine thread) ----
    override fun onDevices(devices: List<DeviceInfo>) {}

    override fun onConnected(camera: CameraInfo, caps: CameraCaps, settings: List<CameraSetting>) {
        this.camera = camera; this.caps = caps; this.settings = settings
        session.setLiveView(true)
    }

    override fun onDisconnected(reason: String?) {
        camera = null; caps = null; settings = emptyList(); frame = null; liveView = false; recording = false
        reason?.let { message = it }
    }

    override fun onSettings(settings: List<CameraSetting>) { this.settings = settings }
    override fun onCapabilities(caps: CameraCaps) { this.caps = caps }
    override fun onLiveView(running: Boolean) { liveView = running }
    override fun onRecording(recording: Boolean) { this.recording = recording }

    override fun onFrame(frame: LiveFrame) {
        val bmp = Bitmap.createBitmap(frame.width.toInt(), frame.height.toInt(), Bitmap.Config.ARGB_8888)
        bmp.copyPixelsFromBuffer(ByteBuffer.wrap(frame.rgba))
        this.frame = bmp
    }

    override fun onCaptured(path: String?, label: String) { message = if (path != null) "Saved $label" else "$label stored on the camera card" }

    override fun onLog(level: LogLevel, message: String) {
        log = (log + message).takeLast(200)
        if (level == LogLevel.ERROR) this.message = message
    }

    override fun onCleared() {
        usb.unregister()
        session.disconnect()
        session.destroy()
    }
}
