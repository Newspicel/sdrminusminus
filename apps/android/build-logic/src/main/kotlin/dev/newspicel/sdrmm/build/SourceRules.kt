package dev.newspicel.sdrmm.build

object SourceRules {
    private const val EM_DASH = '\u2014'
    private val LITERALS = Regex("\"\"\"[\\s\\S]*?\"\"\"|\"(?:\\\\.|[^\"\\\\\\n])*\"|'(?:\\\\.|[^'\\\\\\n])*'")

    fun violations(
        path: String,
        text: String,
    ): List<String> {
        val found = mutableListOf<String>()
        text.lines().forEachIndexed { index, line ->
            if (line.contains(EM_DASH)) found += "$path:${index + 1}: em dash"
        }
        when {
            path.endsWith(".kt") || path.endsWith(".kts") -> found += kotlin(path, text)
            path.endsWith(".xml") -> found += xml(path, text)
        }
        return found
    }

    private fun kotlin(
        path: String,
        text: String,
    ): List<String> {
        val main = path.contains("/src/main/") || path.startsWith("src/main/")
        val code = LITERALS.replace(text) { match -> "\n".repeat(match.value.count { it == '\n' }) }
        val found = mutableListOf<String>()
        code.lines().forEachIndexed { index, line ->
            val at = "$path:${index + 1}"
            if (line.contains("//")) found += "$at: comment"
            if (line.contains("/*")) found += "$at: comment"
            if (main && line.contains("!!")) found += "$at: !!"
        }
        return found
    }

    private fun xml(
        path: String,
        text: String,
    ): List<String> =
        text.lines().mapIndexedNotNull { index, line ->
            if (line.contains("<!--")) "$path:${index + 1}: comment" else null
        }
}
