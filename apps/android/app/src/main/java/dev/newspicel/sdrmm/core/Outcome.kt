package dev.newspicel.sdrmm.core

import dev.newspicel.sdrmm.ffi.CoreException

sealed interface Outcome<out T> {
    data class Ok<T>(
        val value: T,
    ) : Outcome<T>

    data class Failed(
        val error: CoreException,
    ) : Outcome<Nothing>
}

inline fun <T> catching(block: () -> T): Outcome<T> = try {
    Outcome.Ok(block())
} catch (error: CoreException) {
    Outcome.Failed(error)
}
