package com.ezrdp.app

import java.io.BufferedInputStream
import java.io.DataInputStream
import java.io.IOException
import java.math.BigInteger
import java.net.InetSocketAddress
import java.net.Socket
import java.nio.ByteBuffer
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.MessageDigest
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPublicKeySpec
import javax.crypto.KeyAgreement

/**
 * "Trust this phone" pairing. Must match server/src/pairing.rs:
 * phone commits to its key, PC sends its key, phone reveals its key, both show a 6-digit
 * code made from the two keys, the user clicks Allow on the PC, both derive the device key.
 */
object Pairing {
    class Result(val pcId: ByteArray, val pcName: String, val deviceId: ByteArray, val key: ByteArray)

    class Refused(val reason: Int) : IOException("Pairing refused ($reason)")

    private fun sha256(vararg parts: ByteArray): ByteArray {
        val md = MessageDigest.getInstance("SHA-256")
        for (p in parts) md.update(p)
        return md.digest()
    }

    private fun fixed32(n: BigInteger): ByteArray {
        val b = n.toByteArray()
        return when {
            b.size == 32 -> b
            b.size > 32 -> b.copyOfRange(b.size - 32, b.size)
            else -> ByteArray(32 - b.size) + b
        }
    }

    fun code(phonePub: ByteArray, pcPub: ByteArray): Int {
        val d = sha256("EzRDP code v1".toByteArray(), phonePub, pcPub)
        val n = ByteBuffer.wrap(d, 0, 4).int.toLong() and 0xffffffffL
        return (n % 1_000_000L).toInt()
    }

    fun formatCode(c: Int) = "%03d %03d".format(c / 1000, c % 1000)

    /**
     * Pairs with the PC at [host]. Blocking. [onCode] is called with the code to show while
     * waiting for the person to click Allow on the PC. [socketOut] receives the socket so the
     * caller can cancel by closing it.
     */
    fun pair(host: String, port: Int, phoneName: String, socketOut: (Socket) -> Unit, onCode: (String) -> Unit): Result {
        val socket = Socket()
        socketOut(socket)
        socket.use { s ->
            s.tcpNoDelay = true
            s.connect(InetSocketAddress(host, port), 4000)
            s.soTimeout = 15000
            val input = DataInputStream(BufferedInputStream(s.getInputStream()))
            val output = s.getOutputStream()
            val hello = readHello(input)
            if (hello.paused) throw Refused(Msg.RESULT_PAUSED)

            val gen = KeyPairGenerator.getInstance("EC")
            gen.initialize(ECGenParameterSpec("secp256r1"))
            val pair = gen.generateKeyPair()
            val pub = pair.public as ECPublicKey
            val phonePub = byteArrayOf(4) + fixed32(pub.w.affineX) + fixed32(pub.w.affineY)

            writePlain(output, Msg.C_PAIR_START, sha256(phonePub) + phoneName.toByteArray(Charsets.UTF_8))
            val (type, body) = readPlain(input)
            if (type == Msg.S_PAIR_RESULT) throw Refused(body[0].toInt())
            if (type != Msg.S_PAIR_KEY || body.size != 65 || body[0].toInt() != 4) throw IOException("Bad pairing reply")
            val pcPub = body
            writePlain(output, Msg.C_PAIR_KEY, phonePub)

            onCode(formatCode(code(phonePub, pcPub)))
            s.soTimeout = 130_000 // time for the person to walk to the PC and click Allow
            val (rtype, result) = readPlain(input)
            if (rtype != Msg.S_PAIR_RESULT) throw IOException("Bad pairing reply")
            if (result[0].toInt() != Msg.RESULT_OK || result.size < 17) throw Refused(result[0].toInt())

            val point = ECPoint(BigInteger(1, pcPub.copyOfRange(1, 33)), BigInteger(1, pcPub.copyOfRange(33, 65)))
            val pcKey = KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(point, pub.params))
            val agree = KeyAgreement.getInstance("ECDH")
            agree.init(pair.private)
            agree.doPhase(pcKey, true)
            val shared = agree.generateSecret()
            val key = sha256("EzRDP device key v1".toByteArray(), shared, phonePub, pcPub)
            return Result(hello.pcId, hello.name, result.copyOfRange(1, 17), key)
        }
    }
}
