package dev.newspicel.sdrmm

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import dev.newspicel.sdrmm.pair.PairLinkIntake
import dev.newspicel.sdrmm.ui.AppRoot
import dev.newspicel.sdrmm.ui.theme.SdrmmTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val app = application as SdrmmApp
        setContent { SdrmmTheme { AppRoot(app) } }
        if (savedInstanceState == null) handle(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    private fun handle(intent: Intent?) {
        val link = intent?.dataString ?: return
        if (!PairLinkIntake.isPairLink(link)) return
        val startup = (application as SdrmmApp).startup.value
        (startup as? Startup.Ready)?.graph?.let { graph ->
            graph.intake.offer(link)
            graph.navigator.showPair()
        }
    }
}
