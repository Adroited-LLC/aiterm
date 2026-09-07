package com.adroited.aiterm.ui

import org.junit.Assert.*
import org.junit.Test

class ConversationDraftStoreTest {
    @Test fun switchingSessionsPreservesEachDraftAndClearingReleasesThem() {
        val store = ConversationDraftStore()
        store.forSession("a").text = "first draft"
        store.forSession("b").text = "second draft"
        assertEquals("first draft", store.forSession("a").text)
        assertEquals("second draft", store.forSession("b").text)
        store.clear()
        assertEquals("", store.forSession("a").text)
    }
}
