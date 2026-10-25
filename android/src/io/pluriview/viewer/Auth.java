package io.pluriview.viewer;

import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.SecureRandom;
import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;

/**
 * Signs each request with the pairing token instead of sending it. The PC
 * checks the HMAC and rejects reused nonces, so a captured request cannot be
 * replayed. Times come from the monotonic clock: the PC never compares them
 * with its own clock, only with earlier requests from this phone.
 */
final class Auth {
    private static final SecureRandom RANDOM = new SecureRandom();
    private Auth() {}

    static String header(String token, String method, String path, String client) {
        byte[] nonceBytes = new byte[16];
        RANDOM.nextBytes(nonceBytes);
        String nonce = hex(nonceBytes);
        long time = System.nanoTime() / 1_000_000L;
        return "Pluriview-HMAC t=" + time + ", n=" + nonce + ", m=" + signature(token, method, path, client, time, nonce);
    }

    static String signature(String token, String method, String path, String client, long time, String nonce) {
        try {
            Mac mac = Mac.getInstance("HmacSHA256");
            mac.init(new SecretKeySpec(token.getBytes(StandardCharsets.US_ASCII), "HmacSHA256"));
            String message = "pluriview-hmac-v1\n" + method + "\n" + path + "\n" + client + "\n" + time + "\n" + nonce;
            return hex(mac.doFinal(message.getBytes(StandardCharsets.US_ASCII)));
        } catch (GeneralSecurityException error) {
            throw new IllegalStateException("HMAC-SHA256 unavailable", error);
        }
    }

    private static String hex(byte[] bytes) {
        StringBuilder text = new StringBuilder(bytes.length * 2);
        for (byte value : bytes) text.append(Character.forDigit((value >> 4) & 15, 16)).append(Character.forDigit(value & 15, 16));
        return text.toString();
    }
}
