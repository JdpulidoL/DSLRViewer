package app.lumen.tether

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.material3.windowsizeclass.ExperimentalMaterial3WindowSizeClassApi
import androidx.compose.material3.windowsizeclass.calculateWindowSizeClass
import app.lumen.tether.ui.LumenApp
import app.lumen.tether.ui.LumenTheme

class MainActivity : ComponentActivity() {
    private val vm: LumenViewModel by viewModels()

    @OptIn(ExperimentalMaterial3WindowSizeClassApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            LumenTheme {
                LumenApp(vm, calculateWindowSizeClass(this).widthSizeClass)
            }
        }
    }
}
