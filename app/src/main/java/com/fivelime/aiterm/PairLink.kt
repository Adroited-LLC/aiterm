package com.fivelime.aiterm

import android.net.Uri

/** The QR: `aiterm://pair?v=1&p=<port>&t=<token>&n=<name>&h=<addr>&h=<addr>…`.
 *  Hosts repeat, best first. Anything we don't understand is a refusal, not
 *  a guess — the payload decides what we trust. */
data class PairLink(val hosts: List<String>, val port: Int, val token: String, val name: String, val fingerprint: String) {
    val candidates: List<String> get() = hosts.map { "https://$it:$port" }

    companion object {
        fun parse(raw: String): PairLink? {
            val uri = runCatching { Uri.parse(raw.trim()) }.getOrNull() ?: return null
            if (uri.scheme != "aiterm" || uri.host != "pair") return null
            if (uri.getQueryParameter("v") != "1") return null
            val hosts = uri.getQueryParameters("h").filter { it.isNotBlank() }
            val port = uri.getQueryParameter("p")?.toIntOrNull() ?: return null
            val token = uri.getQueryParameter("t")?.takeIf { it.isNotBlank() } ?: return null
            val name = uri.getQueryParameter("n")?.takeIf { it.isNotBlank() } ?: "Desktop"
            val fp = uri.getQueryParameter("f")?.takeIf { it.length == 64 } ?: return null
            if (hosts.isEmpty()) return null
            return PairLink(hosts, port, token, name, fp)
        }
    }
}
