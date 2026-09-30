package dev.newspicel.sdrmm.build

import com.android.build.api.variant.ApplicationAndroidComponentsExtension
import org.gradle.api.Plugin
import org.gradle.api.Project

class RustCorePlugin : Plugin<Project> {
    override fun apply(project: Project) {
        val extension = project.extensions.create("rustCore", RustCoreExtension::class.java)
        val components = project.extensions.getByType(ApplicationAndroidComponentsExtension::class.java)
        components.onVariants(components.selector().all()) { variant ->
            val suffix = variant.name.replaceFirstChar { it.uppercase() }
            val rust =
                project.tasks.register("rustLibs$suffix", RustLibsTask::class.java) {
                    workspaceDir.set(extension.workspaceDir)
                    ndkDirectory.set(components.sdkComponents.ndkDirectory)
                    abis.set(extension.abis)
                }
            variant.sources.jniLibs?.addGeneratedSourceDirectory(rust, RustLibsTask::outputDir)
            val bindgen =
                project.tasks.register("uniffi$suffix", UniffiBindgenTask::class.java) {
                    workspaceDir.set(extension.workspaceDir)
                    cratePackage.set(extension.cratePackage)
                    libraryName.set(extension.libraryName)
                }
            variant.sources.kotlin?.addGeneratedSourceDirectory(bindgen, UniffiBindgenTask::outputDir)
        }
        val root = project.rootProject.layout.projectDirectory
        val rules =
            project.tasks.register("checkSourceRules", SourceRulesTask::class.java) {
                baseDir.set(root)
                sources.from(
                    project.fileTree("src") { include("**/*.kt") },
                    project.fileTree(project.projectDir) { include("*.gradle.kts") },
                    project.fileTree("src/main") { include("**/*.xml") },
                    project.fileTree(root) { include("*.gradle.kts") },
                    project.fileTree(root.dir("build-logic")) { include("*.gradle.kts", "src/**/*.kt") },
                )
                report.set(project.layout.buildDirectory.file("reports/source-rules.txt"))
            }
        project.tasks.named("check") { dependsOn(rules) }
    }
}
