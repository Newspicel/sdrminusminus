package dev.newspicel.sdrmm.core

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import dev.newspicel.sdrmm.RealCore
import dev.newspicel.sdrmm.ffi.CoreException
import dev.newspicel.sdrmm.ffi.LatLon
import dev.newspicel.sdrmm.ffi.LinkState
import dev.newspicel.sdrmm.ffi.NavApp
import dev.newspicel.sdrmm.secrets.KeystoreCipher
import dev.newspicel.sdrmm.secrets.KeystoreVault
import dev.newspicel.sdrmm.testing.Samples
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RealCore
@RunWith(AndroidJUnit4::class)
class CoreLoadsOnDeviceTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val dir = File(context.noBackupFilesDir, "vault-core-test")

    @After
    fun cleanUp() {
        dir.deleteRecursively()
    }

    @Test
    fun the_native_core_loads_with_the_keystore_vault() = runTest {
        val created = UniffiCoreGateway.create(context, KeystoreVault(dir, KeystoreCipher(ALIAS)))
        assertThat(created).isInstanceOf(Outcome.Ok::class.java)
        val core = (created as Outcome.Ok).value
        val about = core.about()
        assertThat(about.protocol).isGreaterThan(0u)
        assertThat(about.coreVersion).isNotEmpty()
        assertThat(core.licenses().map { it.name }).contains("uniffi")
        assertThat(core.savedServers()).isEqualTo(Outcome.Ok(emptyList<Any>()))
        assertThat(core.forgetServer("0123456789abcdef")).isEqualTo(Outcome.Ok(Unit))
        assertThat(core.navUri(LatLon(52.52, 13.405), NavApp.CAR)).isEqualTo("geo:52.520000,13.405000")
        val offer = core.parsePairLink(Samples.link())
        assertThat(offer).isInstanceOf(Outcome.Ok::class.java)
        val parsed = (offer as Outcome.Ok).value
        assertThat(parsed.hosts).containsExactly("192.168.1.20:8443")
        assertThat(parsed.code).isEqualTo("48210937")
        assertThat(parsed.fingerprint).isEqualTo(Samples.PIN)
        assertThat(parsed.serverName).isEqualTo("Bench")
        val short = core.parsePairLink(Samples.link(code = "4821093"))
        assertThat((short as Outcome.Failed).error).isInstanceOf(CoreException.InvalidLink::class.java)
        val pump = core.run(backgroundScope)
        withTimeout(PATIENCE_MS) { core.link.first { it is LinkState.Offline } }
        assertThat(pump.isActive).isTrue()
    }

    private companion object {
        const val ALIAS = "sdrmm.test.core"
        const val PATIENCE_MS = 5_000L
    }
}
