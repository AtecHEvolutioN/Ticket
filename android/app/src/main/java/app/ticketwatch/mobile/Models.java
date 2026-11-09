package app.ticketwatch.mobile;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.List;

/** Plain data shared by the catalog, page checks, and watch list. */
public final class Models {
    private Models() { }

    public static final class Artist {
        public String id = "";
        public String name = "";
    }

    public static final class Event {
        public String id = "";
        public String name = "";
        public String url = "";
        public String date = "";
        public String city = "";
        public String country = "";
        public String venue = "";
        public String venueId = "";
        /** Discovery catalog status only; never evidence of available inventory. */
        public String status = "";
        public List<TicketData.PriceRange> priceRanges = new ArrayList<>();
        public boolean demo;
    }

    public static final class EventPage {
        public List<Event> events = new ArrayList<>();
        public int totalPages;
        public int totalElements;
        public int page;
    }

    public static final class Watch {
        public String id = "";
        public String name = "";
        public String url = "";
        public String date = "";
        public String city = "";
        public String country = "";
        public String venue = "";
        public String venueId = "";
        public String artist = "";
        public String venuePresetId = "";
        public String availableSelector = "";
        public String unavailableSelector = "";
        public String state = "UNKNOWN";
        public String detail = "Not checked yet.";
        public String lastDefinite = "UNKNOWN";
        public boolean selectorsVerified;
        public boolean demo;
        public long checkedAt;
        public List<TicketData.PriceRange> priceRanges = new ArrayList<>();
        public TicketData.Profile offerProfile = new TicketData.Profile();
        public TicketData.Snapshot ticketSnapshot = new TicketData.Snapshot();

        public static Watch fromEvent(Event event, String artist) {
            return Models.fromEvent(event, artist);
        }
    }

    public static Watch fromEvent(Event event, String artist) {
        if (event == null) throw new IllegalArgumentException("Choose an event first.");
        Watch watch = new Watch();
        watch.url = DiscoveryClient.normalizePublicUrl(event.url);
        if (watch.url.isEmpty()) throw new IllegalArgumentException("This event has no supported public HTTPS link.");
        watch.id = watchId(watch.url);
        watch.name = safe(event.name);
        watch.date = safe(event.date);
        watch.city = safe(event.city);
        watch.country = safe(event.country);
        watch.venue = safe(event.venue);
        watch.venueId = safe(event.venueId);
        watch.artist = safe(artist);
        watch.demo = event.demo;
        watch.priceRanges = TicketData.readPriceRanges(TicketData.priceRangesToJson(event.priceRanges));
        return watch;
    }

    /** The canonical public URL, not an API credential or session URL, defines identity. */
    public static String watchId(String canonicalUrl) {
        try {
            byte[] bytes = MessageDigest.getInstance("SHA-256")
                    .digest(safe(canonicalUrl).getBytes(StandardCharsets.UTF_8));
            StringBuilder hex = new StringBuilder("watch-");
            for (byte value : bytes) {
                hex.append(Character.forDigit((value >>> 4) & 15, 16));
                hex.append(Character.forDigit(value & 15, 16));
            }
            return hex.toString();
        } catch (Exception ignored) {
            throw new IllegalStateException("Watch identity could not be created.");
        }
    }

    static String safe(String value) { return value == null ? "" : value; }
}
