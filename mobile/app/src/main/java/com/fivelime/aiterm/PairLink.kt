package com.fivelime.aiterm

import android.net.Uri

/** The QR: `aiterm://pair?v=1&p=<port>&t=<token>&n=<name>&h=<addr>&h=<addr>…`.
 *  Hosts repeat, best first. Anything we don't understand is a refusal, not
 *  a guess — the payload decides what we trust.
 *
 *  A combined QR (one code pairing either phone app) carries the gateway's
 *  fields under `p`/`f`/`s` and ours under `tp`/`tt`/`tf` — when `tt` is
 *  present it is our payload and `p`/`f` belong to the other app. */
data class PairLink(
    val hosts: List<String>, val port: Int, val token: String, val name: String, val fingerprint: String,
    /** iroh node id — the reach-from-anywhere address; "" when the desktop predates it. */
    val iroh: String = "",
) {
    val candidates: List<String> get() = hosts.map { "https://$it:$port" }

    companion object {
        fun parse(raw: String): PairLink? {
            val uri = runCatching { Uri.parse(raw.trim()) }.getOrNull() ?: return null
            if (uri.scheme != "aiterm" || uri.host != "pair") return null
            if (uri.getQueryParameter("v") != "1") return null
            val hosts = uri.getQueryParameters("h").filter { it.isNotBlank() }
            val combined = uri.getQueryParameter("tt")?.isNotBlank() == true
            val port = uri.getQueryParameter(if (combined) "tp" else "p")?.toIntOrNull() ?: return null
            val token = uri.getQueryParameter(if (combined) "tt" else "t")?.takeIf { it.isNotBlank() } ?: return null
            val name = uri.getQueryParameter("n")?.takeIf { it.isNotBlank() } ?: "Desktop"
            val fp = uri.getQueryParameter(if (combined) "tf" else "f")?.takeIf { it.length == 64 } ?: return null
            if (hosts.isEmpty()) return null
            val iroh = uri.getQueryParameter("z")?.takeIf { it.length == 64 } ?: ""
            return PairLink(hosts, port, token, name, fp, iroh)
        }
    }
}
