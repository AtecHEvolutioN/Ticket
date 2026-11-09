package app.ticketwatch.mobile;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;
import org.json.JSONObject;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

/** Small, synchronous private store; every operation sees the latest committed state. */
public final class WatchStore {
    private static final Object LOCK = new Object();
    private static final int MAX_WATCHES = 500;
    private static final int MAX_STORAGE_BYTES = 4 * 1024 * 1024;
    private final SharedPreferences preferences;
    private final VenueStore venues;

    public WatchStore(Context context) {
        preferences = context.getApplicationContext().getSharedPreferences("watch_list", Context.MODE_PRIVATE);
        venues = new VenueStore(context);
    }

    public synchronized List<Models.Watch> all() {
        synchronized (LOCK) { return read(); }
    }

    public synchronized Models.Watch get(String id) {
        synchronized (LOCK) {
            for (Models.Watch watch : read()) if (watch.id.equals(id)) return watch;
            return null;
        }
    }

    public synchronized void save(Models.Watch watch) {
        if (watch == null) throw new IllegalArgumentException("Choose a watch first.");
        synchronized (LOCK) {
            String normalized = DiscoveryClient.normalizePublicUrl(watch.url);
            if (normalized.isEmpty() && !(watch.demo && Models.safe(watch.url).isEmpty())) {
                throw new IllegalArgumentException("Use a supported public HTTPS event link.");
            }
            watch.url = normalized;
            if (Models.safe(watch.id).isEmpty()) watch.id = Models.watchId(normalized);
            try { venues.register(watch); }
            catch (IllegalStateException ignored) { /* Optional venue storage must not block monitoring. */ }
            if (venues.selected(watch) == null) watch.venuePresetId = "";
            List<Models.Watch> watches = read();
            for (int i = 0; i < watches.size(); i++) {
                if (watches.get(i).id.equals(watch.id)) {
                    Models.Watch previous = watches.get(i);
                    if (watch.venuePresetId.isEmpty() && !previous.venuePresetId.isEmpty()) {
                        // Discovery re-saves metadata without a selection field. Clearing has a separate atomic action.
                        watch.venuePresetId = previous.venuePresetId;
                        if (venues.selected(watch) == null) watch.venuePresetId = "";
                    }
                    boolean evidenceChanged = !previous.url.equals(watch.url)
                            || !Models.safe(previous.availableSelector).equals(Models.safe(watch.availableSelector))
                            || !Models.safe(previous.unavailableSelector).equals(Models.safe(watch.unavailableSelector))
                            || previous.selectorsVerified != watch.selectorsVerified || previous.demo != watch.demo
                            || !TicketData.sameProfile(previous.offerProfile, watch.offerProfile);
                    if (evidenceChanged) {
                        watch.state = "UNKNOWN";
                        watch.lastDefinite = "UNKNOWN";
                        watch.checkedAt = 0;
                        watch.detail = "Check the updated page rules to establish a new baseline.";
                        watch.ticketSnapshot = new TicketData.Snapshot();
                    } else {
                        // Saving event metadata or a stale edit must not erase a newer observation.
                        watch.state = previous.state;
                        watch.lastDefinite = previous.lastDefinite;
                        watch.checkedAt = previous.checkedAt;
                        watch.detail = previous.detail;
                        watch.ticketSnapshot = previous.ticketSnapshot;
                    }
                    watches.set(i, watch);
                    write(watches);
                    return;
                }
            }
            if (watches.size() >= MAX_WATCHES) throw new IllegalStateException("The watch list is full. Remove a watch first.");
            watches.add(watch);
            write(watches);
        }
    }

    /** Venue references do not change availability, parser verification, or observation state. */
    synchronized boolean setVenuePreset(String watchId, String presetId) {
        synchronized (LOCK) {
            List<Models.Watch> watches = read();
            for (Models.Watch watch : watches) if (watch.id.equals(watchId)) {
                if (Models.safe(presetId).isEmpty()) { watch.venuePresetId=""; write(watches); return true; }
                for (VenueStore.Preset preset : venues.presetsFor(watch)) if (preset.id.equals(presetId)) {
                    watch.venuePresetId=preset.id; write(watches); return true;
                }
                return false;
            }
            return false;
        }
    }

    public synchronized void remove(String id) {
        synchronized (LOCK) {
            List<Models.Watch> watches = read();
            for (int i = watches.size() - 1; i >= 0; i--) if (watches.get(i).id.equals(id)) watches.remove(i);
            write(watches);
        }
    }

    /** Initial available is quiet; uncertain checks never erase the definite baseline. */
    public synchronized boolean observe(String id, String state, String detail) {
        return observe(id, state, detail, null);
    }

    /** Offer details are evidence from this check only; uncertain checks clear them. */
    public synchronized boolean observe(String id, String state, String detail, TicketData.Snapshot snapshot) {
        synchronized (LOCK) {
            List<Models.Watch> watches = read();
            for (Models.Watch watch : watches) {
                if (!watch.id.equals(id)) continue;
                String next = normalizeState(state);
                boolean transition = "UNAVAILABLE".equals(watch.lastDefinite) && "AVAILABLE".equals(next);
                watch.state = next;
                watch.detail = bounded(detail, 1000);
                watch.checkedAt = System.currentTimeMillis();
                TicketData.Snapshot observed = snapshot == null ? null
                        : TicketData.snapshotFromJson(TicketData.snapshotToJson(snapshot));
                if (observed != null) observed.observedAt = watch.checkedAt;
                watch.ticketSnapshot = acceptedSnapshot(watch, observed);
                if ("AVAILABLE".equals(next) || "UNAVAILABLE".equals(next)) watch.lastDefinite = next;
                write(watches);
                return transition;
            }
            return false;
        }
    }

    private List<Models.Watch> read() {
        List<Models.Watch> result = new ArrayList<>();
        try {
            String raw = preferences.getString("watches", "[]");
            if (raw == null || raw.length() > MAX_STORAGE_BYTES) return result;
            JSONArray array = new JSONArray(raw);
            for (int i = 0; i < Math.min(array.length(), MAX_WATCHES); i++) {
                JSONObject item = array.optJSONObject(i);
                if (item == null) continue;
                Models.Watch watch = new Models.Watch();
                watch.id = field(item, "id", 128);
                watch.demo = item.optBoolean("demo", false);
                String savedUrl = field(item, "url", 4096);
                watch.url = DiscoveryClient.normalizePublicUrl(savedUrl);
                if (watch.id.isEmpty() || (watch.url.isEmpty() && !(watch.demo && savedUrl.isEmpty()))) continue;
                watch.name = field(item, "name", 500);
                watch.date = field(item, "date", 100);
                watch.city = field(item, "city", 200);
                watch.country = field(item, "country", 100);
                watch.venue = field(item, "venue", 500);
                watch.venueId = field(item, "venueId", 150);
                watch.venuePresetId = field(item, "venuePresetId", 80);
                if (venues.selected(watch) == null) watch.venuePresetId = "";
                watch.artist = field(item, "artist", 300);
                watch.availableSelector = field(item, "availableSelector", 2000);
                watch.unavailableSelector = field(item, "unavailableSelector", 2000);
                watch.state = normalizeState(field(item, "state", 30));
                watch.detail = field(item, "detail", 1000);
                watch.lastDefinite = normalizeState(field(item, "lastDefinite", 30));
                if (!"AVAILABLE".equals(watch.lastDefinite) && !"UNAVAILABLE".equals(watch.lastDefinite)) watch.lastDefinite = "UNKNOWN";
                watch.selectorsVerified = item.optBoolean("selectorsVerified", false);
                watch.checkedAt = Math.max(0, item.optLong("checkedAt", 0));
                watch.priceRanges = TicketData.readPriceRanges(item.optJSONArray("priceRanges"));
                watch.offerProfile = TicketData.profileFromJson(item.optJSONObject("offerProfile"));
                watch.ticketSnapshot = acceptedSnapshot(watch, TicketData.snapshotFromJson(item.optJSONObject("ticketSnapshot")));
                result.add(watch);
            }
        } catch (Exception ignored) { /* Malformed local data never creates a positive observation. */ }
        return result;
    }

    private void write(List<Models.Watch> watches) {
        try {
            JSONArray array = new JSONArray();
            for (Models.Watch watch : watches) {
                JSONObject item = new JSONObject();
                item.put("id", bounded(watch.id, 128));
                item.put("url", bounded(watch.url, 4096));
                item.put("name", bounded(watch.name, 500));
                item.put("date", bounded(watch.date, 100));
                item.put("city", bounded(watch.city, 200));
                item.put("country", bounded(watch.country, 100));
                item.put("venue", bounded(watch.venue, 500));
                item.put("venueId", bounded(watch.venueId, 150));
                item.put("venuePresetId", venues.selected(watch) == null ? "" : bounded(watch.venuePresetId, 80));
                item.put("artist", bounded(watch.artist, 300));
                item.put("availableSelector", bounded(watch.availableSelector, 2000));
                item.put("unavailableSelector", bounded(watch.unavailableSelector, 2000));
                item.put("state", normalizeState(watch.state));
                item.put("detail", bounded(watch.detail, 1000));
                item.put("lastDefinite", normalizeState(watch.lastDefinite));
                item.put("selectorsVerified", watch.selectorsVerified);
                item.put("demo", watch.demo);
                item.put("checkedAt", Math.max(0, watch.checkedAt));
                item.put("priceRanges", TicketData.priceRangesToJson(watch.priceRanges));
                item.put("offerProfile", TicketData.profileToJson(watch.offerProfile));
                item.put("ticketSnapshot", TicketData.snapshotToJson(acceptedSnapshot(watch, watch.ticketSnapshot)));
                array.put(item);
            }
            String serialized = array.toString();
            if (serialized.getBytes(StandardCharsets.UTF_8).length > MAX_STORAGE_BYTES) throw new IllegalStateException();
            if (!preferences.edit().putString("watches", serialized).commit()) throw new IllegalStateException();
        } catch (Exception ignored) {
            throw new IllegalStateException("The watch list could not be saved. Try again.");
        }
    }

    private static String field(JSONObject object, String key, int limit) {
        return object.isNull(key) ? "" : bounded(object.optString(key, ""), limit);
    }

    private static TicketData.Snapshot acceptedSnapshot(Models.Watch watch, TicketData.Snapshot input) {
        if (input == null || !input.verified || input.observedAt <= 0 || input.observedAt != watch.checkedAt) return new TicketData.Snapshot();
        boolean builtin = !watch.demo && "web".equals(input.source) && GenericTicketParser.ADAPTER.equals(input.adapter);
        if (!("AVAILABLE".equals(watch.state) || (builtin && "UNKNOWN".equals(watch.state) && input.offers.isEmpty()))) return new TicketData.Snapshot();
        boolean demo = watch.demo && "demo".equals(input.source);
        TicketData.Profile profile = watch.offerProfile;
        boolean web = !watch.demo && "web".equals(input.source) && watch.selectorsVerified
                && !Models.safe(watch.availableSelector).isEmpty() && !Models.safe(watch.unavailableSelector).isEmpty()
                && profile != null && profile.verified
                && !Models.safe(profile.rowSelector).isEmpty() && !Models.safe(profile.availableSelector).isEmpty();
        if (!demo && !web && !builtin) return new TicketData.Snapshot();
        return TicketData.snapshotFromJson(TicketData.snapshotToJson(input));
    }

    private static String bounded(String value, int limit) {
        String result = Models.safe(value);
        return result.length() > limit ? result.substring(0, limit) : result;
    }

    private static String normalizeState(String value) {
        String state = Models.safe(value).toUpperCase(Locale.ROOT);
        return "AVAILABLE".equals(state) || "UNAVAILABLE".equals(state) || "BLOCKED".equals(state) ? state : "UNKNOWN";
    }
}
