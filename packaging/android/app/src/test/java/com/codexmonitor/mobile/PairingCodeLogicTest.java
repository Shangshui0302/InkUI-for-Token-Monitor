package com.codexmonitor.mobile;

import java.security.KeyPairGenerator;
import java.security.SecureRandom;
import java.security.Signature;
import java.security.spec.ECGenParameterSpec;

/** Small dependency-free regression test run by the Android build script. */
public final class PairingCodeLogicTest {
    public static void main(String[] args) {
        PairingCode codes = new PairingCode(null);
        String initial = codes.current();
        require(PairingCode.isValid(initial), "generated code format");

        PairingCode.Reservation first = codes.reserve(initial);
        require(first != null, "first reserve succeeds");
        require(codes.reserve(initial) == null, "parallel reserve cannot reuse a code");
        codes.release(first);
        PairingCode.Reservation retry = codes.reserve(initial);
        require(retry != null, "failed handshake can release a code");
        require(codes.consume(retry), "successful pairing consumes its reservation");
        require(!initial.equals(codes.current()), "consumption rotates the code");
        require(codes.reserve(initial) == null, "consumed code cannot be reused");

        String beforeRefresh = codes.current();
        PairingCode.Reservation revoked = codes.reserve(beforeRefresh);
        require(revoked != null, "reservation before refresh");
        codes.refresh();
        require(!codes.consume(revoked), "refresh immediately revokes a reservation");
        require(codes.reserve(beforeRefresh) == null, "refresh immediately invalidates old code");
        require(!PairingCode.isValid("1234567!") && !PairingCode.isValid("1234567"),
                "only eight ASCII alphanumerics are accepted");
        byte[] transcript = PairingCode.transcript("initiator-id", "ed:aa", "p256:ii",
                "acceptor-id", "ed:bb", "p256:jj", "test-network", "010203");
        String proof = PairingCode.proof("Ab3dE7qK", "initiator", transcript);
        require("d3c36aa348cdf4d9e468cd777935154e3151affe24684827d1ecb4aae026b94c".equals(proof),
                "Rust-compatible transcript and HMAC-SHA256 vector");
        PairingCode codeProof = new PairingCode("Ab3dE7qK");
        require(codeProof.reserveProof(proof, "initiator", transcript) != null,
                "valid code proof reserves current code");
        PairingCode wrongCodeProof = new PairingCode("Ab3dE7qK");
        require(wrongCodeProof.reserveProof(proof, "acceptor", transcript) == null,
                "role-substituted proof is rejected");
        try {
            KeyPairGenerator generator = KeyPairGenerator.getInstance("EC");
            generator.initialize(new ECGenParameterSpec("secp256r1"), new SecureRandom());
            java.security.KeyPair keys = generator.generateKeyPair();
            Signature signer = Signature.getInstance("SHA256withECDSA");
            signer.initSign(keys.getPrivate());
            signer.update(transcript);
            byte[] challengeSignature = signer.sign();
            Signature verifier = Signature.getInstance("SHA256withECDSA");
            verifier.initVerify(keys.getPublic());
            verifier.update(PairingCode.acceptanceBytes(transcript));
            require(!verifier.verify(challengeSignature),
                    "challenge signature cannot authenticate PairAccepted");
        } catch (Exception error) {
            throw new AssertionError("signature-domain separation test", error);
        }
        System.out.println("PairingCodeLogicTest: passed");
    }

    private static void require(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
}
