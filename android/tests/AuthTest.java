package io.pluriview.viewer;

public final class AuthTest {
    private static void check(boolean condition, String message) { if (!condition) throw new AssertionError(message); }

    public static void main(String[] args) {
        // Shared with src/remote_view/auth.rs so both sides sign identically.
        String vector = Auth.signature("0123456789abcdef0123456789abcdef", "GET", "/v2/video",
            "00000000000000000000000000000001", 42, "ffeeddccbbaa99887766554433221100");
        check("16031fcf008b7e0e89af754288a13fa5e9771529bf633b703b326b0fd70a9e18".equals(vector), "Signature differs from the PC");

        String first = Auth.header("secret", "GET", "/v2/video", "client");
        String second = Auth.header("secret", "GET", "/v2/video", "client");
        check(first.matches("Pluriview-HMAC t=[0-9]+, n=[0-9a-f]{32}, m=[0-9a-f]{64}"), "Malformed header");
        check(!first.equals(second), "Nonce reused");
        check(!first.contains("secret"), "Token sent on the wire");
        System.out.println("Auth tests passed");
    }
}
