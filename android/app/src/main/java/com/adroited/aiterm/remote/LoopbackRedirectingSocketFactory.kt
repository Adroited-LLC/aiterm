package com.adroited.aiterm.remote

import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketAddress
import javax.net.SocketFactory

/** Keeps TLS hostname verification aimed at the paired desktop while the
 * underlying TCP connection enters a local encrypted tunnel. */
internal class LoopbackRedirectingSocketFactory(private val localPort: Int) : SocketFactory() {
    override fun createSocket(): Socket = RedirectingSocket(localPort)

    override fun createSocket(host: String, port: Int): Socket =
        createSocket().apply { connect(InetSocketAddress(host, port)) }

    override fun createSocket(host: String, port: Int, localHost: InetAddress, localPort: Int): Socket =
        createSocket().apply {
            bind(InetSocketAddress(localHost, localPort))
            connect(InetSocketAddress(host, port))
        }

    override fun createSocket(host: InetAddress, port: Int): Socket =
        createSocket().apply { connect(InetSocketAddress(host, port)) }

    override fun createSocket(
        address: InetAddress,
        port: Int,
        localAddress: InetAddress,
        localPort: Int,
    ): Socket = createSocket().apply {
        bind(InetSocketAddress(localAddress, localPort))
        connect(InetSocketAddress(address, port))
    }
}

private class RedirectingSocket(private val localPort: Int) : Socket() {
    override fun connect(endpoint: SocketAddress?) = connect(endpoint, 0)

    override fun connect(endpoint: SocketAddress?, timeout: Int) {
        val loopback = InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1))
        super.connect(InetSocketAddress(loopback, localPort), timeout)
    }
}
