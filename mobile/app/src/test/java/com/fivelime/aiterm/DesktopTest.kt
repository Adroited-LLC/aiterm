package com.fivelime.aiterm

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The stored shape of a desktop across versions: what an older entry
 *  reads as, and what a status answer parses to. */
class DesktopTest {
    private val json = Json { ignoreUnknownKeys = true }

    @Test fun anEntryStoredBeforeTheFlagFollowsTheDesktopsOrder() {
        val raw = """{"baseUrl":"https://192.168.1.5:8877","token":"tok","name":"Desk","fingerprint":"${"f".repeat(64)}"}"""
        val d = json.decodeFromString(Desktop.serializer(), raw)
        assertFalse(d.roadOrderCustom)
        assertEquals(DEFAULT_ROAD_ORDER, d.roadOrder)
        val own = json.decodeFromString(Desktop.serializer(), raw.dropLast(1) + ""","roadOrder":["iroh","lan","vpn","relay"],"roadOrderCustom":true}""")
        assertTrue(own.roadOrderCustom)
        assertEquals(listOf("iroh", "lan", "vpn", "relay"), own.roadOrder)
        // Round trip keeps the flag.
        assertEquals(own, json.decodeFromString(Desktop.serializer(), json.encodeToString(Desktop.serializer(), own)))
    }

    @Test fun aStatusWithAWaitingDraftAndAnOrderParses() {
        val raw = """{"api":1,"name":"office","version":"0.10.66","hosts":[],"iroh":null,"relay":null,
            "relay_enroll":{"digest":"CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk"},"relay_error":null,
            "roads":{"lan":false,"vpn":false,"relay":true,"iroh":true},"road_order":["iroh","relay","lan","vpn"]}"""
        val s = json.decodeFromString(Status.serializer(), raw)
        assertNull(s.relay)
        assertEquals("CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk", s.relay_enroll?.digest)
        assertEquals(32, PairLink.decodeBase64Url(s.relay_enroll!!.digest)!!.size)
        assertEquals(listOf("iroh", "relay", "lan", "vpn"), s.road_order)
        assertTrue(Roads.isComplete(s.road_order!!))
        // An older desktop's status: nothing to sign, no order to follow.
        val old = json.decodeFromString(Status.serializer(), """{"api":1,"name":"office","version":"0.10.0"}""")
        assertNull(old.relay_enroll)
        assertNull(old.road_order)
    }
}
