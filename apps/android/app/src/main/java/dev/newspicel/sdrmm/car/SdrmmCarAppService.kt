package dev.newspicel.sdrmm.car

import androidx.car.app.CarAppService
import androidx.car.app.Session
import androidx.car.app.SessionInfo
import androidx.car.app.validation.HostValidator
import dev.newspicel.sdrmm.BuildConfig
import dev.newspicel.sdrmm.R
import dev.newspicel.sdrmm.SdrmmApp

class SdrmmCarAppService : CarAppService() {
    override fun createHostValidator(): HostValidator = if (BuildConfig.DEBUG) {
        HostValidator.ALLOW_ALL_HOSTS_VALIDATOR
    } else {
        HostValidator.Builder(applicationContext).addAllowedHosts(R.array.car_hosts).build()
    }

    override fun onCreateSession(sessionInfo: SessionInfo): Session = CarSession((application as? SdrmmApp)?.startup?.value)
}
