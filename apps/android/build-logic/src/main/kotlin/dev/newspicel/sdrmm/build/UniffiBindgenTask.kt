package dev.newspicel.sdrmm.build

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.FileSystemOperations
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import java.io.ByteArrayOutputStream
import java.io.File
import javax.inject.Inject

abstract class UniffiBindgenTask : DefaultTask() {
    @get:Internal abstract val workspaceDir: DirectoryProperty

    @get:Input abstract val cratePackage: Property<String>

    @get:Input abstract val libraryName: Property<String>

    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @get:Inject abstract val exec: ExecOperations

    @get:Inject abstract val fileOps: FileSystemOperations

    init {
        doNotTrackState("cargo tracks Rust staleness")
    }

    @TaskAction
    fun generate() {
        val workspace = workspaceDir.get().asFile
        val out = outputDir.get().asFile
        fileOps.delete { delete(out) }
        cargo(workspace, "rustc", "--locked", "-p", cratePackage.get(), "--lib", "--crate-type", "cdylib")
        val library = File(targetDirectory(workspace), "debug/${hostLibraryFile(libraryName.get(), System.getProperty("os.name"))}")
        cargo(
            workspace,
            "run",
            "--locked",
            "--quiet",
            "-p",
            "xtask",
            "--bin",
            "uniffi-bindgen",
            "--",
            "generate",
            "--language",
            "kotlin",
            "--out-dir",
            out.path,
            "--no-format",
            library.path,
        )
    }

    private fun cargo(
        workspace: File,
        vararg args: String,
    ) {
        exec.exec {
            workingDir = workspace
            commandLine(listOf("cargo") + args)
        }
    }

    private fun targetDirectory(workspace: File): String {
        val stdout = ByteArrayOutputStream()
        exec.exec {
            workingDir = workspace
            commandLine("cargo", "metadata", "--format-version", "1", "--no-deps")
            standardOutput = stdout
        }
        return parseTargetDirectory(stdout.toString(Charsets.UTF_8))
            ?: throw GradleException("cargo metadata names no target directory")
    }

    companion object {
        private val TARGET_DIRECTORY = Regex("\"target_directory\"\\s*:\\s*\"((?:\\\\.|[^\"\\\\])*)\"")

        fun parseTargetDirectory(metadata: String): String? =
            TARGET_DIRECTORY
                .find(metadata)
                ?.groupValues
                ?.get(1)
                ?.replace("\\\\", "\\")

        fun hostLibraryFile(
            library: String,
            osName: String,
        ): String =
            when {
                osName.startsWith("Mac") -> "lib$library.dylib"
                osName.startsWith("Windows") -> "$library.dll"
                else -> "lib$library.so"
            }
    }
}
