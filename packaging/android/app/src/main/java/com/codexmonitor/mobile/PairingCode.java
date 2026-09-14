package com.codexmonitor.mobile;

import java.security.SecureRandom;
import java.security.MessageDigest;
import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;
import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.nio.charset.StandardCharsets;

/** Thread-safe, single-use pairing code with revocable in-flight reservations. */
final class PairingCode {
    private static final char[] ALPHABET =
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz".toCharArray();
    private static final char[] DIGITS = "0123456789".toCharArray();
    private static final char[] UPPER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".toCharArray();
    private static final char[] LOWER = "abcdefghijklmnopqrstuvwxyz".toCharArray();
    private final SecureRandom random = new SecureRandom();

    private String value;
    private long generation;
    private long reservationId;
    private boolean reserved;

    PairingCode(String initialValue) {
        value = isValid(initialValue) ? initialValue : generate();
    }

    synchronized String current() {
        return value;
    }

    synchronized String refresh() {
        String oldValue = value;
        generation++;
        reservationId++;
        reserved = false;
        value = generateDifferentFrom(oldValue);
        return value;
    }

    synchronized Reservation reserve(String candidate) {
        if (reserved || !value.equals(candidate) || !isValid(candidate)) {
            return null;
        }
        reserved = true;
        reservationId++;
        return new Reservation(generation, reservationId);
    }

    synchronized Reservation reserveProof(String proof, String role, byte[] transcript) {
        if (reserved || !constantTimeEquals(proof, proof(value, role, transcript))) return null;
        reserved = true;
        reservationId++;
        return new Reservation(generation, reservationId);
    }

    synchronized String reservedProof(String role, byte[] transcript) {
        if (!reserved) throw new IllegalStateException("pairing code is not reserved");
        return proof(value, role, transcript);
    }

    static String proof(String code, String role, byte[] transcript) {
        try {
            ByteArrayOutputStream input = new ByteArrayOutputStream();
            input.write("codex-monitor/pair-code-proof-v2:".getBytes(StandardCharsets.UTF_8));
            input.write(role.getBytes(StandardCharsets.UTF_8));
            input.write(0);
            input.write(transcript);
            Mac mac = Mac.getInstance("HmacSHA256");
            mac.init(new SecretKeySpec(code.getBytes(StandardCharsets.US_ASCII), "HmacSHA256"));
            byte[] digest = mac.doFinal(input.toByteArray());
            StringBuilder hex = new StringBuilder(digest.length * 2);
            for (byte b : digest) hex.append(String.format(java.util.Locale.ROOT, "%02x", b & 0xff));
            return hex.toString();
        } catch (Exception error) {
            throw new IllegalStateException("HMAC-SHA256 unavailable", error);
        }
    }

    static boolean constantTimeEquals(String left, String right) {
        if (left == null || right == null) return false;
        try {
            return MessageDigest.isEqual(hexBytes(left), hexBytes(right));
        } catch (IllegalArgumentException invalid) {
            return false;
        }
    }

    private static byte[] hexBytes(String value) {
        if (value == null || (value.length() & 1) != 0) throw new IllegalArgumentException();
        byte[] result = new byte[value.length() / 2];
        for (int i = 0; i < result.length; i++) {
            int high = Character.digit(value.charAt(i * 2), 16);
            int low = Character.digit(value.charAt(i * 2 + 1), 16);
            if (high < 0 || low < 0) throw new IllegalArgumentException();
            result[i] = (byte) ((high << 4) | low);
        }
        return result;
    }

    static byte[] transcript(String initiatorId, String initiatorPublicKey,
            String initiatorPairingKey, String acceptorId, String acceptorPublicKey,
            String acceptorPairingKey, String networkId, String challenge) {
        try {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            DataOutputStream output = new DataOutputStream(bytes);
            output.write("codex-monitor/pair-transcript-v2".getBytes(StandardCharsets.UTF_8));
            for (String field : new String[] {initiatorId, initiatorPublicKey,
                    initiatorPairingKey, acceptorId, acceptorPublicKey,
                    acceptorPairingKey, networkId, challenge}) {
                byte[] encoded = field.getBytes(StandardCharsets.UTF_8);
                output.writeInt(encoded.length);
                output.write(encoded);
            }
            output.flush();
            return bytes.toByteArray();
        } catch (Exception impossible) {
            throw new IllegalStateException(impossible);
        }
    }

    static byte[] acceptanceBytes(byte[] transcript) {
        try {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            DataOutputStream output = new DataOutputStream(bytes);
            output.write("codex-monitor/pair-acceptance-v2".getBytes(StandardCharsets.UTF_8));
            output.writeInt(transcript.length);
            output.write(transcript);
            output.flush();
            return bytes.toByteArray();
        } catch (Exception impossible) {
            throw new IllegalStateException(impossible);
        }
    }

    synchronized boolean consume(Reservation reservation) {
        if (!owns(reservation)) {
            return false;
        }
        String oldValue = value;
        generation++;
        reserved = false;
        value = generateDifferentFrom(oldValue);
        return true;
    }

    synchronized void release(Reservation reservation) {
        if (owns(reservation)) {
            reserved = false;
        }
    }

    private boolean owns(Reservation reservation) {
        return reservation != null && reserved
                && reservation.generation == generation
                && reservation.id == reservationId;
    }

    static boolean isValid(String candidate) {
        if (candidate == null || candidate.length() != 8) {
            return false;
        }
        for (int i = 0; i < candidate.length(); i++) {
            char c = candidate.charAt(i);
            if (!(c >= '0' && c <= '9') && !(c >= 'A' && c <= 'Z')
                    && !(c >= 'a' && c <= 'z')) {
                return false;
            }
        }
        return true;
    }

    private String generate() {
        char[] result = new char[8];
        result[0] = DIGITS[random.nextInt(DIGITS.length)];
        result[1] = UPPER[random.nextInt(UPPER.length)];
        result[2] = LOWER[random.nextInt(LOWER.length)];
        for (int i = 3; i < result.length; i++) {
            result[i] = ALPHABET[random.nextInt(ALPHABET.length)];
        }
        for (int i = result.length - 1; i > 0; i--) {
            int j = random.nextInt(i + 1);
            char tmp = result[i];
            result[i] = result[j];
            result[j] = tmp;
        }
        return new String(result);
    }

    private String generateDifferentFrom(String previous) {
        String next;
        do { next = generate(); } while (next.equals(previous));
        return next;
    }

    static final class Reservation {
        private final long generation;
        private final long id;

        Reservation(long generation, long id) {
            this.generation = generation;
            this.id = id;
        }
    }
}
