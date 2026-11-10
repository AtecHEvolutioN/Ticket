package app.ticketwatch.mobile;

import android.content.Context;
import android.content.SharedPreferences;
import org.json.JSONArray;
import org.json.JSONObject;
import java.nio.charset.StandardCharsets;

/** Separate from observations so background checks cannot overwrite selection preferences. */
public final class TicketPreferencesStore {
    private static final Object LOCK = new Object();
    private static final int MAX_DOCUMENT = 256 * 1024;
    private final SharedPreferences preferences;

    public TicketPreferencesStore(Context context) {
        preferences = context.getApplicationContext().getSharedPreferences("ticket_preferences", Context.MODE_PRIVATE);
    }

    public TicketPreferences defaults() {
        synchronized (LOCK) { return TicketPreferences.fromJson(read().optJSONObject("defaults")); }
    }

    public TicketPreferences effective(String eventUrl) {
        synchronized (LOCK) {
            JSONObject document = read();
            JSONObject override = find(document, eventUrl);
            return TicketPreferences.fromJson(override == null ? document.optJSONObject("defaults") : override.optJSONObject("preferences"));
        }
    }

    public boolean hasOverride(String eventUrl) {
        synchronized (LOCK) { return find(read(), eventUrl) != null; }
    }

    public void saveDefaults(TicketPreferences value) {
        TicketPreferences checked = value.checkedCopy();
        synchronized (LOCK) {
            JSONObject document = read();
            try { document.put("defaults", checked.toJson()); }
            catch (Exception error) { throw new IllegalStateException("Could not save ticket defaults."); }
            write(document);
        }
    }

    public void saveOverride(String eventUrl, TicketPreferences value) {
        String url = eventUrl(eventUrl);
        TicketPreferences checked = value.checkedCopy();
        synchronized (LOCK) {
            JSONObject document = read();
            JSONArray entries = document.optJSONArray("event_overrides");
            JSONObject existing = find(document, url);
            if (existing == null && entries.length() >= 100) throw new IllegalStateException("The event preference list is full.");
            try {
                if (existing == null) {
                    existing = new JSONObject();
                    existing.put("event_url", url);
                    entries.put(existing);
                }
                existing.put("preferences", checked.toJson());
            } catch (Exception error) { throw new IllegalStateException("Could not save event preferences."); }
            write(document);
        }
    }

    public void clearOverride(String eventUrl) {
        String url = eventUrl(eventUrl);
        synchronized (LOCK) {
            JSONObject document = read();
            JSONArray entries = document.optJSONArray("event_overrides");
            for (int index = entries.length() - 1; index >= 0; index--)
                if (PageSignals.sameEvent(url, entries.optJSONObject(index).optString("event_url"))) entries.remove(index);
            write(document);
        }
    }

    public String exportJson() {
        synchronized (LOCK) {
            try {
                JSONObject document = read();
                String formatted = document.toString(2) + "\n";
                return formatted.getBytes(StandardCharsets.UTF_8).length <= MAX_DOCUMENT
                        ? formatted : document.toString();
            }
            catch (Exception error) { throw new IllegalStateException("Could not export ticket preferences."); }
        }
    }

    /** An exportable event needs the same root/www public host rules as Discovery. */
    public static boolean supportsEvent(String value) {
        String normalized = DiscoveryClient.normalizePublicUrl(value == null ? "" : value);
        return !normalized.isEmpty() && PageSignals.sameEvent(normalized, normalized);
    }

    private JSONObject read() {
        String raw = preferences.getString("document", "");
        try {
            if (raw == null || raw.getBytes(StandardCharsets.UTF_8).length > MAX_DOCUMENT) throw new IllegalArgumentException();
            if (raw.isEmpty()) {
                JSONObject document = new JSONObject();
                document.put("schema_version", TicketPreferences.SCHEMA_VERSION);
                document.put("defaults", new TicketPreferences().toJson());
                document.put("event_overrides", new JSONArray());
                return document;
            }
            JSONObject document = new JSONObject(raw);
            TicketPreferences.requireFields(document, "schema_version", "defaults", "event_overrides");
            Object version = document.get("schema_version");
            if (!(version instanceof Integer) || (Integer) version != TicketPreferences.SCHEMA_VERSION) throw new IllegalArgumentException();
            TicketPreferences.fromJson(document.getJSONObject("defaults"));
            JSONArray entries = document.getJSONArray("event_overrides");
            if (entries.length() > 100) throw new IllegalArgumentException();
            java.util.ArrayList<String> seen = new java.util.ArrayList<>();
            for (int index = 0; index < entries.length(); index++) {
                JSONObject entry = entries.getJSONObject(index);
                TicketPreferences.requireFields(entry, "event_url", "preferences");
                String savedUrl = entry.getString("event_url");
                if (!savedUrl.equals(eventUrl(savedUrl))) throw new IllegalArgumentException();
                for (String previous : seen) if (PageSignals.sameEvent(previous, savedUrl)) throw new IllegalArgumentException();
                seen.add(savedUrl);
                TicketPreferences.fromJson(entry.getJSONObject("preferences"));
            }
            return document;
        } catch (Exception error) {
            throw new IllegalStateException("Saved ticket preferences could not be read. They have not been replaced.");
        }
    }

    private JSONObject find(JSONObject document, String url) {
        String canonical = DiscoveryClient.normalizePublicUrl(url == null ? "" : url);
        if (canonical.isEmpty()) return null;
        JSONArray entries = document.optJSONArray("event_overrides");
        for (int index = 0; index < entries.length(); index++) {
            JSONObject entry = entries.optJSONObject(index);
            if (PageSignals.sameEvent(canonical, entry.optString("event_url"))) return entry;
        }
        return null;
    }

    private static String eventUrl(String value) {
        String result = DiscoveryClient.normalizePublicUrl(value == null ? "" : value);
        if (!supportsEvent(result))
            throw new IllegalArgumentException("Choose a Ticketmaster event with a supported public HTTPS link.");
        return result;
    }

    private void write(JSONObject document) {
        String value = document.toString();
        if (value.getBytes(StandardCharsets.UTF_8).length > MAX_DOCUMENT || !preferences.edit().putString("document", value).commit())
            throw new IllegalStateException("Ticket preferences could not be saved. Try again.");
    }
}
