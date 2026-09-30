package dev.newspicel.sdrmm.permissions

import android.Manifest
import android.app.Application
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.common.truth.Truth.assertThat
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(AndroidJUnit4::class)
class PermissionGateTest {
    private val context: Application = ApplicationProvider.getApplicationContext()
    private val reports = mutableListOf<Pair<Need, Boolean>>()
    private val asked = mutableListOf<Need>()
    private val gate =
        PermissionGate(context, mutableMapOf()) { need, allowed -> reports += need to allowed }.also { gate ->
            gate.launch = { need, _ -> asked += need }
        }

    @Test
    @Config(sdk = [37])
    fun asks_in_order_and_moves_on_after_each_answer() {
        gate.requestInOrder(listOf(Need.LocalNetwork, Need.Location))
        assertThat(asked).containsExactly(Need.LocalNetwork)
        gate.answered(Need.LocalNetwork, false)
        assertThat(asked).containsExactly(Need.LocalNetwork, Need.Location).inOrder()
        gate.answered(Need.Location, true)
        assertThat(reports).containsExactly(Need.LocalNetwork to false, Need.Location to true).inOrder()
    }

    @Test
    @Config(sdk = [36])
    fun needs_without_permissions_on_this_sdk_are_allowed_at_once() {
        gate.requestInOrder(listOf(Need.LocalNetwork, Need.Location))
        assertThat(reports).containsExactly(Need.LocalNetwork to true)
        assertThat(asked).containsExactly(Need.Location)
    }

    @Test
    fun granted_needs_are_skipped() {
        shadowOf(context).grantPermissions(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)
        gate.requestInOrder(listOf(Need.Location, Need.Camera))
        assertThat(reports).containsExactly(Need.Location to true)
        assertThat(asked).containsExactly(Need.Camera)
    }

    @Test
    fun two_denials_stop_asking() {
        repeat(2) {
            gate.request(Need.Camera, fromUser = false)
            gate.answered(Need.Camera, false)
        }
        gate.request(Need.Camera, fromUser = false)
        assertThat(asked).containsExactly(Need.Camera, Need.Camera)
        assertThat(reports.last()).isEqualTo(Need.Camera to false)
    }
}
