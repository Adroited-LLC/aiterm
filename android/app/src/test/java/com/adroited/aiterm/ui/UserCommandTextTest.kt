package com.adroited.aiterm.ui

import com.adroited.aiterm.remote.Item
import org.junit.Assert.assertEquals
import org.junit.Test

class UserCommandTextTest {
    private val clear = """
        <command-name>/clear</command-name>
                      <command-message>clear</command-message>
                      <command-args></command-args>
    """.trimIndent()

    @Test fun clearAndOtherCompleteCommandsHaveNoMetadataInTheirDisplay() {
        assertEquals("/clear", userCommandText(clear))
        assertEquals("/clear", userCommandText("› $clear"))
        assertEquals("/login", userCommandText("<command-name>/login</command-name>"))
        assertEquals("/plugin:run first\nsecond <literal>", userCommandText(
            "<command-message>run</command-message><command-name>/plugin:run</command-name>" +
                "<command-args> first\nsecond <literal> </command-args>",
        ))
    }

    @Test fun proseCodeIncompleteAndUnrecognizedXmlRemainUnchanged() {
        for (text in listOf(
            "I saw this: $clear", "$clear\nWhy did this happen?", "```xml\n$clear\n```",
            "`$clear`", "> $clear", "<example>hello</example>",
            "<command-name>/clear", "<command-name></command-name>",
            "<command-name>clear</command-name>", "$clear<other>keep me</other>",
            "$clear<command-name>/login</command-name>", "Plain user prompt",
        )) assertEquals(text, userCommandText(text))
    }

    @Test fun copyAndResendUseTheVisibleCommandWithoutMutatingTheOriginalItem() {
        val item = Item.User("command", clear, 123L)
        assertEquals("/clear", timelineText(SpineTimelineItem.Row(item)))
        assertEquals(clear, item.text)
        assertEquals(clear, timelineText(SpineTimelineItem.Row(Item.AgentText("example", clear, true, 123L))))
    }
}
