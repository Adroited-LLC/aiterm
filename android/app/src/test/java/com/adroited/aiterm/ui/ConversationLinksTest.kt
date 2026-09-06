package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.RemoteSessionChange
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ConversationLinksTest {
    @Test
    fun hostFileLinksStripSourceLocationsAndNormalizePaths() {
        assertEquals("/home/matt/Projects/mojo/README.md", conversationFilePath("/home/matt/Projects/mojo/README.md:12:4"))
        assertEquals("/project/My Report.md", conversationFilePath("/project/docs/../My Report.md:3"))
        assertEquals("/project/My Report.md", conversationFilePath("file:///project/My%20Report.md:3"))
        assertEquals("/project/README.md", conversationFilePath("file://localhost/project/README.md"))
        assertEquals("/project/a+b.md", conversationFilePath("file:///project/a+b.md"))
    }

    @Test
    fun unsupportedAndMalformedFileTargetsAreRejected() {
        listOf(
            "https://example.com/README.md", "README.md", "//server/share/readme.md",
            "file://server/project/README.md", "file://user@localhost/project/README.md",
            "file:///project/README.md?download=1", "file:///project/README.md#heading",
            "file:README.md", "file:///project/%00.txt", "/project/a\nb.txt",
            "file:///project/%GG.txt", "javascript:alert(1)",
        ).forEach { assertNull(it, conversationFilePath(it)) }
    }

    @Test
    fun resolvesOnlyExactLedgerEntriesAndReturnsOriginalRemotePath() {
        val absolute = file("/home/matt/Projects/mojo/README.md")
        val relative = file("docs/My Report.md")
        val files = listOf(absolute, relative)
        assertEquals(absolute, conversationLinkedFile("/home/matt/Projects/mojo/README.md", "/home/matt/Projects/mojo", files))
        assertEquals(relative, conversationLinkedFile("/home/matt/Projects/mojo/docs/My Report.md", "/home/matt/Projects/mojo/", files))
        assertNull(conversationLinkedFile("/another/README.md", "/home/matt/Projects/mojo", files))
        assertNull(conversationLinkedFile("/home/matt/Projects/mojo/README.md.bak", "/home/matt/Projects/mojo", files))
    }

    private fun file(path: String) = RemoteSessionChange(
        path = path, name = path.substringAfterLast('/'), kind = "modified", at = 0, bytes = 10,
    )
}
