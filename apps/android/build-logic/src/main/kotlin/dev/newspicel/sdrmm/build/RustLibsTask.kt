package dev.newspicel.sdrmm.build

import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.FileSystemOperations
import org.gradle.api.provider.ListProperty
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import javax.inject.Inject

abstract class RustLibsTask : DefaultTask() {
    @get:Internal abstract val workspaceDir: DirectoryProperty

    @get:Internal abstract val ndkDirectory: DirectoryProperty

    @get:Input abstract val abis: ListProperty<String>

    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @get:Inject abstract val exec: ExecOperations

    @get:Inject abstract val fileOps: FileSystemOperations

    init {
        doNotTrackState("cargo tracks Rust staleness")
    }

    @TaskAction
    fun build() {
        val staging = temporaryDir.resolve("out")
        fileOps.delete { delete(staging) }
        val ndk = ndkDirectory.get().asFile
        exec.exec {
            workingDir = workspaceDir.get().asFile
            environment("ANDROID_NDK_HOME", ndk.path)
            commandLine(xtaskCommand(staging.path, abis.get()))
        }
        fileOps.sync {
            from(staging.resolve("jniLibs"))
            into(outputDir)
        }
    }

    companion object {
        fun xtaskCommand(
            out: String,
            abis: List<String>,
        ): List<String> = listOf("cargo", "xtask", "mobile", "android", "--out", out) + abis.flatMap { listOf("--abi", it) }
    }
}
