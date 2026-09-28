package dev.newspicel.sdrmm.build

import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property

abstract class RustCoreExtension {
    abstract val workspaceDir: DirectoryProperty
    abstract val cratePackage: Property<String>
    abstract val libraryName: Property<String>
    abstract val abis: ListProperty<String>
}
