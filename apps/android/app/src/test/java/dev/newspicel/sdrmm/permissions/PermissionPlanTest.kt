package dev.newspicel.sdrmm.permissions

import android.Manifest
import com.google.common.truth.Truth.assertThat
import org.junit.Test

class PermissionPlanTest {
    @Test
    fun by_sdk() {
        assertThat(PermissionPlan.permissions(Need.Notifications, 29)).isEmpty()
        assertThat(PermissionPlan.permissions(Need.LocalNetwork, 29)).isEmpty()
        assertThat(PermissionPlan.permissions(Need.Notifications, 33)).containsExactly("android.permission.POST_NOTIFICATIONS")
        assertThat(PermissionPlan.permissions(Need.LocalNetwork, 36)).isEmpty()
        assertThat(PermissionPlan.permissions(Need.LocalNetwork, 37)).containsExactly("android.permission.ACCESS_LOCAL_NETWORK")
        assertThat(PermissionPlan.permissions(Need.Camera, 29)).containsExactly(Manifest.permission.CAMERA)
        assertThat(PermissionPlan.permissions(Need.Location, 37))
            .containsExactly(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)
    }

    @Test
    fun missing_drops_granted() {
        val granted = setOf(Manifest.permission.ACCESS_COARSE_LOCATION)
        assertThat(PermissionPlan.missing(Need.Location, 37, granted)).containsExactly(Manifest.permission.ACCESS_FINE_LOCATION)
        assertThat(PermissionPlan.missing(Need.LocalNetwork, 30, emptySet())).isEmpty()
    }
}
