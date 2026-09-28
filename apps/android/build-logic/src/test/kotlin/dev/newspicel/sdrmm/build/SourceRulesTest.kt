package dev.newspicel.sdrmm.build

import com.google.common.truth.Truth.assertThat
import org.junit.Test

class SourceRulesTest {
    @Test
    fun flagsLineComments() {
        val found = SourceRules.violations("app/src/main/A.kt", "val a = 1\nval b = 2 // two\n")
        assertThat(found).containsExactly("app/src/main/A.kt:2: comment")
    }

    @Test
    fun flagsBlockComments() {
        val found = SourceRules.violations("app/src/test/A.kt", "/* note */\nval a = 1\n")
        assertThat(found).containsExactly("app/src/test/A.kt:1: comment")
    }

    @Test
    fun ignoresSlashesInStrings() {
        val text = "val url = \"https://x\"\nval raw = \"\"\"\n// kept\n\"\"\"\nval c = '/'\n"
        assertThat(SourceRules.violations("app/src/main/A.kt", text)).isEmpty()
    }

    @Test
    fun keepsLineNumbersAfterMultilineStrings() {
        val text = "val raw = \"\"\"\none\ntwo\n\"\"\"\nval b = x!!\n"
        assertThat(SourceRules.violations("app/src/main/A.kt", text)).containsExactly("app/src/main/A.kt:5: !!")
    }

    @Test
    fun flagsXmlComments() {
        val found = SourceRules.violations("app/src/main/res/values/strings.xml", "<resources>\n<!-- x -->\n</resources>\n")
        assertThat(found).containsExactly("app/src/main/res/values/strings.xml:2: comment")
    }

    @Test
    fun flagsNotNullAssertionsOnlyInMain() {
        val text = "val a = b!!\n"
        assertThat(SourceRules.violations("app/src/main/A.kt", text)).containsExactly("app/src/main/A.kt:1: !!")
        assertThat(SourceRules.violations("app/src/test/A.kt", text)).isEmpty()
        assertThat(SourceRules.violations("app/src/sharedTest/A.kt", text)).isEmpty()
    }

    @Test
    fun ignoresNotNullAssertionsInStrings() {
        assertThat(SourceRules.violations("app/src/main/A.kt", "val a = \"hey!!\"\n")).isEmpty()
    }

    @Test
    fun flagsEmDashAnywhere() {
        val dash = "\u2014"
        assertThat(SourceRules.violations("app/src/main/A.kt", "val a = \"x $dash y\"\n"))
            .containsExactly("app/src/main/A.kt:1: em dash")
        assertThat(SourceRules.violations("app/src/main/res/values/strings.xml", "<string>a $dash b</string>\n"))
            .containsExactly("app/src/main/res/values/strings.xml:1: em dash")
    }

    @Test
    fun namesTheXtaskCommand() {
        assertThat(RustLibsTask.xtaskCommand("/out", listOf("arm64-v8a")))
            .containsExactly("cargo", "xtask", "mobile", "android", "--out", "/out", "--abi", "arm64-v8a")
            .inOrder()
    }

    @Test
    fun readsTheCargoTargetDirectory() {
        val metadata = "{\"packages\":[],\"target_directory\":\"/w/target\",\"version\":1}"
        assertThat(UniffiBindgenTask.parseTargetDirectory(metadata)).isEqualTo("/w/target")
        assertThat(UniffiBindgenTask.parseTargetDirectory("{}")).isNull()
        assertThat(UniffiBindgenTask.hostLibraryFile("core", "Mac OS X")).isEqualTo("libcore.dylib")
        assertThat(UniffiBindgenTask.hostLibraryFile("core", "Linux")).isEqualTo("libcore.so")
    }
}
