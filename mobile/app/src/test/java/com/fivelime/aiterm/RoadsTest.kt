package com.fivelime.aiterm

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RoadsTest {
    private fun desk(
        base: String = "https://192.168.1.5:8877",
        cands: List<String> = listOf("https://192.168.1.5:8877", "https://100.100.1.1:8877", "https://203.0.113.9:8877"),
        iroh: String = "", relayHost: String = "", relayPort: Int = 0, order: List<String> = DEFAULT_ROAD_ORDER,
    ) = Desktop(base, "tok", "Desk", cands, "f".repeat(64), iroh, relayHost, relayPort, order)

    @Test fun classification() {
        assertEquals(Road.LAN, Roads.classifyHost("10.0.0.1"))
        assertEquals(Road.LAN, Roads.classifyHost("172.16.0.1"))
        assertEquals(Road.LAN, Roads.classifyHost("172.31.255.255"))
        assertEquals(Road.VPN, Roads.classifyHost("172.32.0.1"))
        assertEquals(Road.VPN, Roads.classifyHost("172.15.0.1"))
        assertEquals(Road.LAN, Roads.classifyHost("192.168.1.5"))
        assertEquals(Road.LAN, Roads.classifyHost("169.254.10.10"))
        assertEquals(Road.VPN, Roads.classifyHost("100.64.0.1"))
        assertEquals(Road.VPN, Roads.classifyHost("100.127.255.255"))
        assertEquals(Road.VPN, Roads.classifyHost("100.128.0.1")) // public, not CGNAT — still not lan
        assertEquals(Road.VPN, Roads.classifyHost("203.0.113.9"))
        assertEquals(Road.VPN, Roads.classifyHost("fd7a:115c:a1e0::1"))
        assertEquals(Road.VPN, Roads.classifyHost("[fc00::1]"))
        assertEquals(Road.LAN, Roads.classifyHost("fe80::1%wlan0"))
        assertEquals(Road.VPN, Roads.classifyHost("2001:db8::1"))
        assertEquals(Road.VPN, Roads.classifyHost("desk.tailnet.ts.net"))
        assertEquals(Road.VPN, Roads.classifyHost("desk.local"))
        assertNull(Roads.classifyHost("127.0.0.1"))
        assertNull(Roads.classifyHost("::1"))
        assertEquals(Road.VPN, Roads.classifyHost("999.1.1.1")) // not dotted-quad → a name
    }

    @Test fun urlHosts() {
        assertEquals("192.168.1.5", Roads.hostOf("https://192.168.1.5:8877"))
        assertEquals("fd7a::1", Roads.hostOf("https://[fd7a::1]:8877"))
        assertEquals("desk.example", Roads.hostOf("https://desk.example:443/x"))
        assertEquals(Road.LAN, Roads.classifyUrl("https://10.1.1.1:8877"))
    }

    @Test fun defaultOrderBuildsLanVpnRelayIroh() {
        val d = desk(iroh = "n".repeat(64), relayHost = "desk-1.relay.example.com", relayPort = 443)
        val c = Roads.candidates(d, "https://127.0.0.1:41234")
        assertEquals(
            listOf(
                Road.LAN to "https://192.168.1.5:8877",
                Road.VPN to "https://100.100.1.1:8877",
                Road.VPN to "https://203.0.113.9:8877",
                Road.RELAY to "https://desk-1.relay.example.com:443",
                Road.IROH to "https://127.0.0.1:41234",
            ),
            c.map { it.road to it.url },
        )
        assertEquals(15L, c.last().patienceSeconds)
        assertEquals(4L, c.first().patienceSeconds)
    }

    @Test fun onlyAWholeOrderIsCompleteEnoughToAdopt() {
        assertTrue(Roads.isComplete(listOf("lan", "vpn", "relay", "iroh")))
        assertTrue(Roads.isComplete(listOf("iroh", "relay", "vpn", "lan")))
        assertFalse(Roads.isComplete(listOf("lan", "vpn", "relay")))
        assertFalse(Roads.isComplete(listOf("lan", "vpn", "relay", "bogus")))
        assertFalse(Roads.isComplete(listOf("lan", "lan", "relay", "iroh")))
        assertFalse(Roads.isComplete(listOf("lan", "vpn", "relay", "iroh", "lan")))
        assertFalse(Roads.isComplete(emptyList()))
    }

    @Test fun orderIsHonouredAndMissingRoadsSkipped() {
        val d = desk(iroh = "n".repeat(64), relayHost = "r.example", relayPort = 443, order = listOf("iroh", "relay", "bogus", "lan"))
        val c = Roads.candidates(d, "https://127.0.0.1:41234")
        assertEquals(listOf(Road.IROH, Road.RELAY, Road.LAN), c.map { it.road })
        assertEquals(listOf(Road.IROH, Road.RELAY, Road.LAN), Roads.order(d.roadOrder))
        assertEquals(0, Roads.rank(d, Road.IROH)); assertEquals(2, Roads.rank(d, Road.LAN))
        assertEquals(Int.MAX_VALUE, Roads.rank(d, Road.VPN))
    }

    @Test fun lastWinnerLeadsItsRoad() {
        val d = desk(base = "https://203.0.113.9:8877", cands = listOf("https://100.100.1.1:8877", "https://203.0.113.9:8877", "https://10.0.0.7:8877"))
        val c = Roads.candidates(d, null)
        assertEquals(listOf("https://10.0.0.7:8877", "https://203.0.113.9:8877", "https://100.100.1.1:8877"), c.map { it.url })
    }

    @Test fun roadsWithNothingToDialContributeNothing() {
        val d = desk(cands = listOf("https://192.168.1.5:8877"))
        assertEquals(listOf(Road.LAN), Roads.candidates(d, null).map { it.road })
        // node id but no bridge url (bridge failed to start): iroh skipped
        assertEquals(listOf(Road.LAN), Roads.candidates(d.copy(iroh = "n".repeat(64)), null).map { it.road })
        // bridge url but no node id: nothing to bridge to
        assertEquals(listOf(Road.LAN), Roads.candidates(d, "https://127.0.0.1:1").map { it.road })
    }

    @Test fun staleLoopbackAndRelayUrlsInTheAddressListAreNotDirect() {
        val d = desk(
            base = "https://127.0.0.1:41234",
            cands = listOf("https://192.168.1.5:8877", "https://127.0.0.1:41234", "https://r.example:443"),
            iroh = "n".repeat(64), relayHost = "r.example", relayPort = 443,
        )
        assertEquals(listOf("https://192.168.1.5:8877"), Roads.directUrls(d))
        val c = Roads.candidates(d, "https://127.0.0.1:41234")
        assertEquals(listOf(Road.LAN, Road.RELAY, Road.IROH), c.map { it.road })
        assertEquals(3, c.size)
    }

    @Test fun offers() {
        val d = desk(cands = listOf("https://192.168.1.5:8877", "https://203.0.113.9:8877"))
        assertTrue(Roads.offers(d, Road.LAN)); assertTrue(Roads.offers(d, Road.VPN))
        assertFalse(Roads.offers(d, Road.RELAY)); assertFalse(Roads.offers(d, Road.IROH))
        assertTrue(Roads.offers(d.copy(relayHost = "r", relayPort = 1), Road.RELAY))
        assertTrue(Roads.offers(d.copy(iroh = "n"), Road.IROH))
        assertFalse(Roads.offers(desk(cands = listOf("https://10.0.0.1:1")), Road.VPN))
    }

    @Test fun relayUrlBracketsIpv6() {
        assertEquals("https://[fd00::1]:443", desk(relayHost = "fd00::1", relayPort = 443).relayUrl)
        assertNull(desk(relayHost = "x", relayPort = 0).relayUrl)
        assertNull(desk(relayHost = "", relayPort = 443).relayUrl)
    }

    @Test fun olderStoredDesktopsLoadWithDefaults() {
        val json = kotlinx.serialization.json.Json { ignoreUnknownKeys = true }
        val d = json.decodeFromString(Desktop.serializer(), """{"baseUrl":"https://10.0.0.1:8877","token":"t","name":"D","fingerprint":"f"}""")
        assertEquals(DEFAULT_ROAD_ORDER, d.roadOrder)
        assertEquals("", d.relayHost); assertEquals(0, d.relayPort); assertEquals("", d.iroh)
        assertEquals(listOf("https://10.0.0.1:8877"), d.candidates)
        val round = json.decodeFromString(Desktop.serializer(), json.encodeToString(Desktop.serializer(), d.copy(roadOrder = listOf("iroh", "lan"), relayHost = "r", relayPort = 5)))
        assertEquals(listOf("iroh", "lan"), round.roadOrder); assertEquals("r", round.relayHost); assertEquals(5, round.relayPort)
    }
}
