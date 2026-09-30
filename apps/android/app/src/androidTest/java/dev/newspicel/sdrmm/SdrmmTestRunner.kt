package dev.newspicel.sdrmm

import android.app.Application
import android.content.Context
import androidx.test.runner.AndroidJUnitRunner
import dev.newspicel.sdrmm.testing.TestSdrmmApp

class SdrmmTestRunner : AndroidJUnitRunner() {
    override fun newApplication(
        cl: ClassLoader,
        className: String,
        context: Context,
    ): Application = super.newApplication(cl, TestSdrmmApp::class.java.name, context)
}
