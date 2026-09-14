package com.codexmonitor.mobile;

import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;

import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.AlgorithmParameters;
import java.security.PrivateKey;
import java.security.Signature;
import java.security.spec.ECGenParameterSpec;
import java.security.spec.ECParameterSpec;
import java.security.spec.X509EncodedKeySpec;
import java.security.interfaces.ECPublicKey;
import java.security.KeyFactory;
import java.security.PublicKey;

/** P-256 signing identity backed by AndroidKeyStore; private key material never leaves it. */
final class NodeIdentity {
    private static final String KEY_ALIAS = "codex-monitor-p256-v1";
    private static final String PREFIX = "p256:";
    private final PrivateKey privateKey;
    private final String publicKey;
    private final String hostId;

    private NodeIdentity(PrivateKey privateKey, byte[] encodedPublicKey) throws Exception {
        this.privateKey = privateKey;
        publicKey = PREFIX + hex(encodedPublicKey);
        byte[] digest = MessageDigest.getInstance("SHA-256").digest(encodedPublicKey);
        hostId = PREFIX + hex(digest).substring(0, 16);
    }

    static NodeIdentity loadOrCreate() throws Exception {
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        if (!store.containsAlias(KEY_ALIAS)) {
            KeyPairGenerator generator = KeyPairGenerator.getInstance("EC", "AndroidKeyStore");
            generator.initialize(new KeyGenParameterSpec.Builder(KEY_ALIAS,
                    KeyProperties.PURPOSE_SIGN)
                    .setAlgorithmParameterSpec(new ECGenParameterSpec("secp256r1"))
                    .setDigests(KeyProperties.DIGEST_SHA256)
                    .setUserAuthenticationRequired(false)
                    .build());
            generator.generateKeyPair();
            store.load(null);
        }
        PrivateKey privateKey = (PrivateKey) store.getKey(KEY_ALIAS, null);
        java.security.cert.Certificate certificate = store.getCertificate(KEY_ALIAS);
        if (privateKey == null || certificate == null || certificate.getPublicKey() == null) {
            throw new IllegalStateException("AndroidKeyStore did not return the P-256 identity");
        }
        return new NodeIdentity(privateKey, certificate.getPublicKey().getEncoded());
    }

    String publicKey() {
        return publicKey;
    }

    String hostId() {
        return hostId;
    }

    String sign(byte[] message) throws Exception {
        Signature signer = Signature.getInstance("SHA256withECDSA");
        signer.initSign(privateKey);
        signer.update(message);
        return hex(signer.sign());
    }

    static boolean verify(String encodedKey, byte[] message, String signatureHex) {
        try {
            byte[] keyBytes = decodePrefixedKey(encodedKey);
            if (keyBytes == null || keyBytes.length > 2048
                    || signatureHex == null || signatureHex.length() < 16
                    || signatureHex.length() > 512) {
                return false;
            }
            PublicKey key = KeyFactory.getInstance("EC")
                    .generatePublic(new X509EncodedKeySpec(keyBytes));
            if (!(key instanceof ECPublicKey) || !isP256((ECPublicKey) key)) {
                return false;
            }
            Signature verifier = Signature.getInstance("SHA256withECDSA");
            verifier.initVerify(key);
            verifier.update(message);
            return verifier.verify(unhex(signatureHex));
        } catch (Exception ignored) {
            return false;
        }
    }

    static String hostIdForPublicKey(String encodedKey) throws Exception {
        byte[] key = decodePrefixedKey(encodedKey);
        if (key == null || key.length > 2048) {
            throw new IllegalArgumentException("invalid P-256 public key");
        }
        PublicKey parsed = KeyFactory.getInstance("EC")
                .generatePublic(new X509EncodedKeySpec(key));
        if (!(parsed instanceof ECPublicKey) || !isP256((ECPublicKey) parsed)) {
            throw new IllegalArgumentException("pairing key is not P-256");
        }
        byte[] digest = MessageDigest.getInstance("SHA-256").digest(key);
        return PREFIX + hex(digest).substring(0, 16);
    }

    private static byte[] decodePrefixedKey(String value) {
        if (value == null || !value.startsWith(PREFIX)
                || value.length() <= PREFIX.length() || value.length() > PREFIX.length() + 4096) {
            return null;
        }
        return unhex(value.substring(PREFIX.length()));
    }

    static String hex(byte[] bytes) {
        char[] digits = "0123456789abcdef".toCharArray();
        char[] output = new char[bytes.length * 2];
        for (int i = 0; i < bytes.length; i++) {
            int value = bytes[i] & 0xff;
            output[i * 2] = digits[value >>> 4];
            output[i * 2 + 1] = digits[value & 0x0f];
        }
        return new String(output);
    }

    private static boolean isP256(ECPublicKey key) throws Exception {
        ECParameterSpec parameters = key.getParams();
        if (parameters == null || parameters.getCurve().getField().getFieldSize() != 256) return false;
        AlgorithmParameters standard = AlgorithmParameters.getInstance("EC");
        standard.init(new ECGenParameterSpec("secp256r1"));
        ECParameterSpec p256 = standard.getParameterSpec(ECParameterSpec.class);
        return parameters.getCurve().equals(p256.getCurve())
                && parameters.getGenerator().equals(p256.getGenerator())
                && parameters.getOrder().equals(p256.getOrder())
                && parameters.getCofactor() == p256.getCofactor();
    }

    static byte[] unhex(String value) {
        if (value == null || (value.length() & 1) != 0) {
            return null;
        }
        byte[] output = new byte[value.length() / 2];
        for (int i = 0; i < output.length; i++) {
            int hi = Character.digit(value.charAt(i * 2), 16);
            int lo = Character.digit(value.charAt(i * 2 + 1), 16);
            if (hi < 0 || lo < 0) {
                return null;
            }
            output[i] = (byte) ((hi << 4) | lo);
        }
        return output;
    }
}
