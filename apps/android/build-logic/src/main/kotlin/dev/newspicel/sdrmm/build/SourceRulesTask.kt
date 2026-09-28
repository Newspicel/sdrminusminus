package dev.newspicel.sdrmm.build

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputFile
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction

abstract class SourceRulesTask : DefaultTask() {
    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    @get:Internal abstract val baseDir: DirectoryProperty

    @get:OutputFile abstract val report: RegularFileProperty

    @TaskAction
    fun check() {
        val base = baseDir.get().asFile
        val found =
            sources.files.sorted().flatMap { file ->
                SourceRules.violations(file.relativeTo(base).invariantSeparatorsPath, file.readText())
            }
        report.get().asFile.writeText(found.joinToString("\n", postfix = if (found.isEmpty()) "" else "\n"))
        if (found.isNotEmpty()) {
            throw GradleException("Source rules broken:\n" + found.joinToString("\n"))
        }
    }
}
