package app.lumen.tether

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbConstants
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbEndpoint
import android.hardware.usb.UsbInterface
import android.hardware.usb.UsbManager
import androidx.core.content.ContextCompat
import uniffi.lumen_android.UsbException
import uniffi.lumen_android.UsbTransport

/**
 * Enumeration, the permission prompt and all USB I/O happen here with Android's own API; Rust only frames PTP
 * (`LumenSession.connectUsb`). libusb over an Android-provided descriptor never got an answer from a Canon T5i,
 * whereas `bulkTransfer` did, so this is the supported path.
 */
class UsbCamera(private val context: Context, private val onOpened: (UsbTransport, name: String) -> Unit, private val onError: (String) -> Unit) {
    private val manager = context.getSystemService(Context.USB_SERVICE) as UsbManager
    private var transport: BulkTransport? = null

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            if (intent.action != ACTION_PERMISSION) return
            @Suppress("DEPRECATION")
            val device = intent.getParcelableExtra<UsbDevice>(UsbManager.EXTRA_DEVICE) ?: return
            if (intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)) open(device)
            else onError("USB permission denied for ${device.productName ?: "camera"}")
        }
    }

    fun register() {
        ContextCompat.registerReceiver(context, receiver, IntentFilter(ACTION_PERMISSION), ContextCompat.RECEIVER_NOT_EXPORTED)
    }

    fun unregister() {
        runCatching { context.unregisterReceiver(receiver) }
        close()
    }

    /** Attached devices that expose a Still Image (PTP) interface. */
    fun cameras(): List<UsbDevice> = manager.deviceList.values.filter { device ->
        (0 until device.interfaceCount).any { device.getInterface(it).interfaceClass == UsbConstants.USB_CLASS_STILL_IMAGE }
    }

    fun request(stale: UsbDevice) {
        // The camera gets a new bus address whenever it is power-cycled or replugged, so a device object taken
        // from an older scan cannot be opened any more: look the same vid:pid up again.
        val device = cameras().firstOrNull { it.vendorId == stale.vendorId && it.productId == stale.productId }
        if (device == null) {
            onError("The camera is no longer connected")
            return
        }
        if (manager.hasPermission(device)) {
            open(device)
            return
        }
        // FLAG_MUTABLE is required: the system fills in EXTRA_DEVICE / EXTRA_PERMISSION_GRANTED.
        val pending = PendingIntent.getBroadcast(
            context, 0, Intent(ACTION_PERMISSION).setPackage(context.packageName), PendingIntent.FLAG_MUTABLE,
        )
        manager.requestPermission(device, pending)
    }

    private fun open(device: UsbDevice) {
        close()
        val conn = manager.openDevice(device)
        if (conn == null) {
            onError("Could not open ${device.productName ?: "the camera"}")
            return
        }
        val t = BulkTransport.create(manager, device, conn)
        if (t == null) {
            conn.close()
            onError("No PTP interface on ${device.productName ?: "the device"}")
            return
        }
        transport = t
        onOpened(t, device.productName ?: "USB camera %04x:%04x".format(device.vendorId, device.productId))
    }

    /** Call after the Rust side has disconnected. */
    fun close() {
        transport?.close()
        transport = null
    }

    private companion object {
        const val ACTION_PERMISSION = "app.lumen.tether.USB_PERMISSION"
    }
}

/** PTP pipe over `UsbDeviceConnection.bulkTransfer`. Chunks stay at 16 KiB, the size every Android version accepts. */
private class BulkTransport(
    private val manager: UsbManager,
    private val device: UsbDevice,
    private val conn: UsbDeviceConnection,
    private val intf: UsbInterface,
    private val bulkIn: UsbEndpoint,
    private val bulkOut: UsbEndpoint,
    private val interrupt: UsbEndpoint?,
) : UsbTransport {

    override fun write(data: ByteArray) {
        var sent = 0
        while (sent < data.size) {
            val n = minOf(CHUNK, data.size - sent)
            val chunk = if (sent == 0 && n == data.size) data else data.copyOfRange(sent, sent + n)
            val r = conn.bulkTransfer(bulkOut, chunk, chunk.size, WRITE_TIMEOUT_MS)
            if (r < 0) throw failure()
            sent += r
        }
        // A transfer that ends exactly on a packet boundary needs a zero-length packet to terminate it.
        if (data.isNotEmpty() && data.size % bulkOut.maxPacketSize == 0) conn.bulkTransfer(bulkOut, ByteArray(0), 0, WRITE_TIMEOUT_MS)
    }

    override fun read(timeoutMs: UInt): ByteArray {
        val buf = ByteArray(CHUNK)
        val r = conn.bulkTransfer(bulkIn, buf, buf.size, timeoutMs.toInt())
        if (r < 0) throw failure()
        return buf.copyOf(r)
    }

    override fun readEvent(timeoutMs: UInt): ByteArray {
        val ep = interrupt ?: return ByteArray(0)
        val buf = ByteArray(64)
        val r = conn.bulkTransfer(ep, buf, buf.size, timeoutMs.toInt())
        return if (r > 0) buf.copyOf(r) else ByteArray(0)
    }

    override fun describe() = "USB %04x:%04x via Android UsbManager".format(device.vendorId, device.productId)

    /** `bulkTransfer` reports every failure as -1: tell a timeout from an unplugged camera. */
    private fun failure(): UsbException =
        if (manager.deviceList.containsKey(device.deviceName)) UsbException.Timeout() else UsbException.Disconnected()

    fun close() {
        runCatching { conn.releaseInterface(intf) }
        conn.close()
    }

    companion object {
        const val CHUNK = 16384
        const val WRITE_TIMEOUT_MS = 5000

        fun create(manager: UsbManager, device: UsbDevice, conn: UsbDeviceConnection): BulkTransport? {
            val intf = (0 until device.interfaceCount).map { device.getInterface(it) }
                .firstOrNull { it.interfaceClass == UsbConstants.USB_CLASS_STILL_IMAGE } ?: return null
            val eps = (0 until intf.endpointCount).map { intf.getEndpoint(it) }
            val bin = eps.firstOrNull { it.type == UsbConstants.USB_ENDPOINT_XFER_BULK && it.direction == UsbConstants.USB_DIR_IN } ?: return null
            val bout = eps.firstOrNull { it.type == UsbConstants.USB_ENDPOINT_XFER_BULK && it.direction == UsbConstants.USB_DIR_OUT } ?: return null
            val int = eps.firstOrNull { it.type == UsbConstants.USB_ENDPOINT_XFER_INT && it.direction == UsbConstants.USB_DIR_IN }
            if (!conn.claimInterface(intf, true)) return null
            return BulkTransport(manager, device, conn, intf, bin, bout, int)
        }
    }
}
