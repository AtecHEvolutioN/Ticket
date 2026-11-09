package app.ticketwatch.mobile;

import android.content.Context;
import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;

import java.nio.charset.StandardCharsets;
import java.security.KeyStore;

import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/** Device-local credentials encrypted by a non-exportable Android Keystore key. */
public final class SecureStore {
    private static final Object LOCK = new Object();
    private static final String ALIAS = "ticketwatch.credentials.v1";
    private final SharedPreferences preferences;

    public SecureStore(Context context) {
        preferences = context.getApplicationContext().getSharedPreferences("encrypted_credentials", Context.MODE_PRIVATE);
    }

    public void put(String name, String value) throws Exception {
        validateName(name);
        if (value == null || value.trim().isEmpty()) { remove(name); return; }
        if (value.length() > 2048) throw new Exception("The credential is too long.");
        synchronized (LOCK) {
            try {
                Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
                cipher.init(Cipher.ENCRYPT_MODE, key());
                cipher.updateAAD(name.getBytes(StandardCharsets.UTF_8));
                String encoded = Base64.encodeToString(cipher.getIV(), Base64.NO_WRAP) + ":"
                        + Base64.encodeToString(cipher.doFinal(value.trim().getBytes(StandardCharsets.UTF_8)), Base64.NO_WRAP);
                if (!preferences.edit().putString(name, encoded).commit()) throw new IllegalStateException();
            } catch (Exception ignored) {
                throw new Exception("The credential could not be saved securely. Try again.");
            }
        }
    }

    public String get(String name) throws Exception {
        validateName(name);
        synchronized (LOCK) {
            try {
                String encoded = preferences.getString(name, "");
                if (encoded == null || encoded.isEmpty()) return "";
                String[] parts = encoded.split(":", -1);
                if (parts.length != 2) throw new IllegalStateException();
                byte[] iv = Base64.decode(parts[0], Base64.NO_WRAP);
                if (iv.length != 12) throw new IllegalStateException();
                Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
                cipher.init(Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(128, iv));
                cipher.updateAAD(name.getBytes(StandardCharsets.UTF_8));
                return new String(cipher.doFinal(Base64.decode(parts[1], Base64.NO_WRAP)), StandardCharsets.UTF_8);
            } catch (Exception ignored) {
                throw new Exception("The saved credential cannot be unlocked. Re-enter it on this device.");
            }
        }
    }

    public void remove(String name) {
        validateName(name);
        synchronized (LOCK) {
            if (!preferences.edit().remove(name).commit()) {
                throw new IllegalStateException("The saved credential could not be removed. Try again.");
            }
        }
    }

    private static void validateName(String name) {
        if (!"api_key".equals(name) && !"discord_token".equals(name)) {
            throw new IllegalArgumentException("Unsupported credential name.");
        }
    }

    private static SecretKey key() throws Exception {
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        if (store.containsAlias(ALIAS)) return (SecretKey) store.getKey(ALIAS, null);
        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        generator.init(new KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setRandomizedEncryptionRequired(true)
                .build());
        return generator.generateKey();
    }
}
