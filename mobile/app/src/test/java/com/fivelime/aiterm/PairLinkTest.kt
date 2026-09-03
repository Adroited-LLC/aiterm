package com.fivelime.aiterm

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PairLinkTest {
    private val fp = "a".repeat(64)
    private val node = "b".repeat(64)
    private val plain = "aiterm://pair?v=1&p=8877&t=tok&n=Desk&h=192.168.1.5&h=203.0.113.9&f=$fp"
    /** 32 bytes as base64url, no padding (43 chars). */
    private val digest = ByteArray(32) { it.toByte() }
    private val digestB64 = java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(digest)

    @Test fun plainLink() {
        val l = PairLink.parse(plain)!!
        assertEquals(listOf("192.168.1.5", "203.0.113.9"), l.hosts)
        assertEquals(8877, l.port)
        assertEquals("tok", l.token)
        assertEquals("Desk", l.name)
        assertEquals(fp, l.fingerprint)
        assertEquals("", l.iroh)
        assertEquals("", l.relayHost)
        assertEquals(0, l.relayPort)
        assertNull(l.relayAuthorization)
        assertEquals(listOf("https://192.168.1.5:8877", "https://203.0.113.9:8877"), l.candidates)
    }

    @Test fun combinedLinkUsesOurFields() {
        val l = PairLink.parse("aiterm://pair?v=1&p=1&t=theirs&f=${"c".repeat(64)}&h=10.0.0.2&tp=8877&tt=ours&tf=$fp&z=$node")!!
        assertEquals(8877, l.port)
        assertEquals("ours", l.token)
        assertEquals(fp, l.fingerprint)
        assertEquals(node, l.iroh)
        assertEquals(listOf("10.0.0.2"), l.hosts)
    }

    @Test fun thWinsOverH() {
        val l = PairLink.parse("aiterm://pair?v=1&p=1&t=t&f=${"c".repeat(64)}&h=10.0.0.2&h=10.0.0.3&tp=8877&tt=ours&tf=$fp&th=100.100.1.1&th=192.168.1.5")!!
        assertEquals(listOf("100.100.1.1", "192.168.1.5"), l.hosts)
    }

    @Test fun blankThFallsBackToH() {
        val l = PairLink.parse("$plain&th=")!!
        assertEquals(listOf("192.168.1.5", "203.0.113.9"), l.hosts)
    }

    @Test fun relayRouteNeedsBothHalves() {
        val both = PairLink.parse("$plain&tr=desk-1.relay.example.com&tq=443")!!
        assertEquals("desk-1.relay.example.com", both.relayHost)
        assertEquals(443, both.relayPort)
        val hostOnly = PairLink.parse("$plain&tr=desk-1.relay.example.com")!!
        assertEquals("", hostOnly.relayHost); assertEquals(0, hostOnly.relayPort)
        val portOnly = PairLink.parse("$plain&tq=443")!!
        assertEquals("", portOnly.relayHost); assertEquals(0, portOnly.relayPort)
        val badPort = PairLink.parse("$plain&tr=x&tq=70000")!!
        assertEquals("", badPort.relayHost); assertEquals(0, badPort.relayPort)
    }

    @Test fun relayAuthorizationIs32Bytes() {
        val l = PairLink.parse("$plain&ta=$digestB64")!!
        assertArrayEquals(digest, l.relayAuthorization)
        // 31 bytes: ignored
        val short = java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(ByteArray(31))
        assertNull(PairLink.parse("$plain&ta=$short")!!.relayAuthorization)
        // padded: not the alphabet the QR uses
        assertNull(PairLink.parse("$plain&ta=$digestB64=")!!.relayAuthorization)
        // standard base64 with + or /: refused
        assertNull(PairLink.parse("$plain&ta=${"+".repeat(43)}")!!.relayAuthorization)
        assertNull(PairLink.parse("$plain&ta=")!!.relayAuthorization)
    }

    @Test fun refusals() {
        assertNull(PairLink.parse("https://example.com/pair?v=1"))
        assertNull(PairLink.parse("aiterm://pairing?v=1&p=1&t=t&f=$fp&h=x"))
        assertNull(PairLink.parse("aiterm://pair?v=2&p=8877&t=tok&f=$fp&h=x"))
        assertNull(PairLink.parse("aiterm://pair?v=1&t=tok&f=$fp&h=x")) // no port
        assertNull(PairLink.parse("aiterm://pair?v=1&p=8877&f=$fp&h=x")) // no token
        assertNull(PairLink.parse("aiterm://pair?v=1&p=8877&t=tok&f=abc&h=x")) // bad fingerprint
        assertNull(PairLink.parse("aiterm://pair?v=1&p=8877&t=tok&f=$fp")) // no hosts
        assertNull(PairLink.parse("")); assertNull(PairLink.parse("garbage"))
    }

    @Test fun percentDecodingAndDefaults() {
        val l = PairLink.parse("  aiterm://pair?v=1&p=8877&t=tok&h=10.1.2.3&f=$fp&n=My%20Desk+1&z=short  ")!!
        assertEquals("My Desk+1", l.name)
        assertEquals("", l.iroh)
        assertEquals("Desktop", PairLink.parse("aiterm://pair?v=1&p=8877&t=tok&h=10.1.2.3&f=$fp")!!.name)
    }

    @Test fun equalityCoversTheDigest() {
        assertEquals(PairLink.parse("$plain&ta=$digestB64"), PairLink.parse("$plain&ta=$digestB64"))
    }
}
