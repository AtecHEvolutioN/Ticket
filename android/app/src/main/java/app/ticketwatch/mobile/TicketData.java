package app.ticketwatch.mobile;

import org.json.JSONArray;
import org.json.JSONObject;

import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.Currency;
import java.util.List;
import java.util.Locale;

/** Catalog ranges and observed offers stay separate, including quantity and price scope. */
public final class TicketData {
    public static final int MAX_OFFERS = 100;
    private static final int MAX_RANGES = 20;
    private TicketData() { }

    /** Discovery's range category (for example standard) is not an available ticket type. */
    public static final class PriceRange {
        public String type = "", currency = "", min = "", max = "";
    }

    public static final class Offer {
        public String key = "", ticketType = "", section = "", row = "";
        public String price = "", priceText = "", currency = "";
        public String priceScope = "unknown", fees = "unknown";
        public String quantityKind = "unknown", quantityText = "";
        public Integer quantity;
    }

    public static final class Category {
        public String ticketType = "", priceText = "", min = "", max = "", currency = "", section = "", row = "";
        public Integer selectableQuantity;
    }

    public static final class Profile {
        public String rowSelector = "", availableSelector = "", typeSelector = "", sectionSelector = "";
        public String priceSelector = "", quantitySelector = "", currency = "";
        public String decimalSeparator = ".", quantityKind = "unknown", priceScope = "unknown", fees = "unknown";
        public boolean verified;
    }

    public static final class Snapshot {
        public List<Offer> offers = new ArrayList<>();
        public List<Category> categories = new ArrayList<>();
        public String adapter = "";
        public long observedAt;
        public boolean verified;
        public String source = "web", detail = "";
    }

    public static List<PriceRange> readPriceRanges(JSONArray values) {
        List<PriceRange> result = new ArrayList<>();
        if (values == null) return result;
        for (int i = 0; i < Math.min(values.length(), MAX_RANGES); i++) {
            JSONObject item = values.optJSONObject(i);
            if (item == null) continue;
            PriceRange range = new PriceRange();
            range.type = field(item, "type", 80);
            range.currency = currency(item.opt("currency"));
            range.min = decimal(item.opt("min"));
            range.max = decimal(item.opt("max"));
            if (range.currency.isEmpty() || (range.min.isEmpty() && range.max.isEmpty())) continue;
            if ((present(item, "min") && range.min.isEmpty()) || (present(item, "max") && range.max.isEmpty())) continue;
            if (!range.min.isEmpty() && !range.max.isEmpty()
                    && new BigDecimal(range.min).compareTo(new BigDecimal(range.max)) > 0) continue;
            result.add(range);
        }
        return result;
    }

    public static JSONArray priceRangesToJson(List<PriceRange> ranges) {
        JSONArray array = new JSONArray();
        if (ranges == null) return array;
        for (int i = 0; i < Math.min(ranges.size(), MAX_RANGES); i++) {
            PriceRange range = ranges.get(i);
            if (range == null) continue;
            JSONObject item = new JSONObject();
            put(item, "type", bounded(range.type, 80));
            put(item, "currency", currency(range.currency));
            if (!decimal(range.min).isEmpty()) put(item, "min", decimal(range.min));
            if (!decimal(range.max).isEmpty()) put(item, "max", decimal(range.max));
            array.put(item);
        }
        // Round-trip through the strict reader before persisting untrusted mutable objects.
        JSONArray normalized = new JSONArray();
        for (PriceRange range : readPriceRanges(array)) {
            JSONObject item = new JSONObject();
            put(item, "type", range.type);
            put(item, "currency", range.currency);
            if (!range.min.isEmpty()) put(item, "min", range.min);
            if (!range.max.isEmpty()) put(item, "max", range.max);
            normalized.put(item);
        }
        return normalized;
    }

    public static JSONObject profileToJson(Profile value) {
        Profile profile = value == null ? new Profile() : value;
        JSONObject item = new JSONObject();
        put(item, "rowSelector", bounded(profile.rowSelector, 2000));
        put(item, "availableSelector", bounded(profile.availableSelector, 2000));
        put(item, "typeSelector", bounded(profile.typeSelector, 2000));
        put(item, "sectionSelector", bounded(profile.sectionSelector, 2000));
        put(item, "priceSelector", bounded(profile.priceSelector, 2000));
        put(item, "quantitySelector", bounded(profile.quantitySelector, 2000));
        put(item, "currency", currency(profile.currency));
        put(item, "decimalSeparator", ",".equals(profile.decimalSeparator) ? "," : ".");
        put(item, "quantityKind", choice(profile.quantityKind, "listing", "selectable", "remaining"));
        put(item, "priceScope", choice(profile.priceScope, "per_ticket", "total"));
        put(item, "fees", choice(profile.fees, "included", "excluded"));
        put(item, "verified", profile.verified);
        return item;
    }

    public static Profile profileFromJson(JSONObject item) {
        Profile result = new Profile();
        if (item == null) return result;
        result.rowSelector = field(item, "rowSelector", 2000);
        result.availableSelector = field(item, "availableSelector", 2000);
        result.typeSelector = field(item, "typeSelector", 2000);
        result.sectionSelector = field(item, "sectionSelector", 2000);
        result.priceSelector = field(item, "priceSelector", 2000);
        result.quantitySelector = field(item, "quantitySelector", 2000);
        result.currency = currency(item.opt("currency"));
        result.decimalSeparator = ",".equals(item.optString("decimalSeparator", ".")) ? "," : ".";
        result.quantityKind = choice(field(item, "quantityKind", 30), "listing", "selectable", "remaining");
        result.priceScope = choice(field(item, "priceScope", 30), "per_ticket", "total");
        result.fees = choice(field(item, "fees", 30), "included", "excluded");
        result.verified = Boolean.TRUE.equals(item.opt("verified"));
        return result;
    }

    public static boolean sameProfile(Profile first, Profile second) {
        // The encoder inserts fixed keys in a fixed order and normalizes unsupported values.
        return profileToJson(first).toString().equals(profileToJson(second).toString());
    }

    public static JSONObject snapshotToJson(Snapshot value) {
        Snapshot snapshot = value == null ? new Snapshot() : value;
        JSONObject item = new JSONObject();
        put(item, "observedAt", Math.max(0, snapshot.observedAt));
        put(item, "verified", snapshot.verified);
        put(item, "source", "demo".equals(snapshot.source) ? "demo" : "web".equals(snapshot.source) ? "web" : "unknown");
        put(item, "detail", bounded(snapshot.detail, 1000));
        put(item, "adapter", GenericTicketParser.ADAPTER.equals(snapshot.adapter) ? snapshot.adapter : "");
        JSONArray categories = new JSONArray();
        if (snapshot.categories != null) for (int i=0;i<Math.min(snapshot.categories.size(),50);i++) {
            Category c=snapshot.categories.get(i); if(c==null)continue; JSONObject row=new JSONObject();
            put(row,"ticketType",bounded(c.ticketType,240)); put(row,"priceText",bounded(c.priceText,240));
            put(row,"section",bounded(c.section,160)); put(row,"row",bounded(c.row,160));
            put(row,"min",decimal(c.min)); put(row,"max",decimal(c.max)); put(row,"currency",currency(c.currency));
            if(c.selectableQuantity!=null && c.selectableQuantity>=0 && c.selectableQuantity<=100000) put(row,"selectableQuantity",c.selectableQuantity);
            categories.put(row);
        }
        put(item,"categories",categories);
        JSONArray array = new JSONArray();
        if (snapshot.offers != null) {
            for (int i = 0; i < Math.min(snapshot.offers.size(), MAX_OFFERS); i++) {
                Offer offer = snapshot.offers.get(i);
                if (offer == null) continue;
                JSONObject row = new JSONObject();
                put(row, "key", bounded(offer.key, 240));
                put(row, "ticketType", bounded(offer.ticketType, 240));
                put(row, "section", bounded(offer.section, 160));
                put(row, "row", bounded(offer.row, 160));
                put(row, "price", decimal(offer.price));
                put(row, "priceText", bounded(offer.priceText, 240));
                put(row, "currency", currency(offer.currency));
                put(row, "priceScope", choice(offer.priceScope, "per_ticket", "total"));
                put(row, "fees", choice(offer.fees, "included", "excluded"));
                put(row, "quantityKind", choice(offer.quantityKind, "listing", "selectable", "remaining"));
                put(row, "quantityText", bounded(offer.quantityText, 240));
                if (offer.quantity != null && offer.quantity >= 0 && offer.quantity <= 1000000) put(row, "quantity", offer.quantity);
                array.put(row);
            }
        }
        put(item, "offers", array);
        return item;
    }

    public static Snapshot snapshotFromJson(JSONObject item) {
        Snapshot snapshot = new Snapshot();
        if (item == null) return snapshot;
        snapshot.observedAt = safeTime(item.opt("observedAt"));
        snapshot.verified = Boolean.TRUE.equals(item.opt("verified"));
        String source = field(item, "source", 30);
        snapshot.source = "demo".equals(source) || "web".equals(source) ? source : "unknown";
        snapshot.detail = field(item, "detail", 1000);
        snapshot.adapter=GenericTicketParser.ADAPTER.equals(item.optString("adapter")) ? GenericTicketParser.ADAPTER : "";
        JSONArray categories=item.optJSONArray("categories");
        for(int i=0;categories!=null && i<Math.min(categories.length(),50);i++) {
            JSONObject row=categories.optJSONObject(i);if(row==null)continue; Category c=new Category();
            c.ticketType=field(row,"ticketType",240);c.priceText=field(row,"priceText",240);c.section=field(row,"section",160);c.row=field(row,"row",160);c.min=decimal(row.opt("min"));c.max=decimal(row.opt("max"));c.currency=currency(row.opt("currency"));
            if(row.opt("selectableQuantity") instanceof Number)c.selectableQuantity=TicketPageParser.parseQuantity(row.opt("selectableQuantity").toString());
            if(!c.ticketType.isEmpty() && !c.priceText.isEmpty())snapshot.categories.add(c);
        }
        JSONArray offers = item.optJSONArray("offers");
        if (offers == null) return snapshot;
        for (int i = 0; i < Math.min(offers.length(), MAX_OFFERS); i++) {
            JSONObject row = offers.optJSONObject(i);
            if (row == null) continue;
            Offer offer = new Offer();
            offer.key = field(row, "key", 240);
            offer.ticketType = field(row, "ticketType", 240);
            offer.section = field(row, "section", 160);
            offer.row = field(row, "row", 160);
            offer.price = decimal(row.opt("price"));
            offer.priceText = field(row, "priceText", 240);
            offer.currency = currency(row.opt("currency"));
            offer.priceScope = choice(field(row, "priceScope", 30), "per_ticket", "total");
            offer.fees = choice(field(row, "fees", 30), "included", "excluded");
            offer.quantityKind = choice(field(row, "quantityKind", 30), "listing", "selectable", "remaining");
            offer.quantityText = field(row, "quantityText", 240);
            Object quantity = row.opt("quantity");
            if (quantity instanceof Number) {
                String count = quantity.toString();
                if (count.matches("[0-9]{1,7}")) {
                    int parsed = Integer.parseInt(count);
                    if (parsed <= 1000000) offer.quantity = parsed;
                }
            }
            if (offer.ticketType.isEmpty() && offer.section.isEmpty() && offer.row.isEmpty()
                    && offer.price.isEmpty() && offer.priceText.isEmpty() && offer.quantity == null && offer.quantityText.isEmpty()) continue;
            snapshot.offers.add(offer);
        }
        return snapshot;
    }

    /** Decimal text only: never infer an amount from currency strings, ranges, or binary arithmetic. */
    private static String decimal(Object value) {
        if (!(value instanceof Number) && !(value instanceof String)) return "";
        String raw = value.toString();
        if (!raw.matches("[0-9]{1,12}(?:\\.[0-9]{1,6})?")) return "";
        try { return new BigDecimal(raw).stripTrailingZeros().toPlainString(); }
        catch (NumberFormatException ignored) { return ""; }
    }

    private static String currency(Object value) {
        if (!(value instanceof String)) return "";
        String code = ((String) value).trim().toUpperCase(Locale.ROOT);
        if (!code.matches("[A-Z]{3}") || "XXX".equals(code) || "XTS".equals(code)) return "";
        try { return Currency.getInstance(code).getCurrencyCode(); }
        catch (IllegalArgumentException ignored) { return ""; }
    }

    private static String choice(String value, String... allowed) {
        for (String option : allowed) if (option.equals(value)) return option;
        return "unknown";
    }

    private static boolean present(JSONObject item, String key) { return item.has(key) && !item.isNull(key); }
    private static String field(JSONObject item, String key, int limit) {
        Object raw = item.opt(key);
        return raw instanceof String ? bounded((String) raw, limit) : "";
    }
    private static String bounded(String value, int limit) {
        String safe = Models.safe(value);
        return safe.length() <= limit ? safe : safe.substring(0, limit);
    }
    private static long safeTime(Object value) {
        if (!(value instanceof Number)) return 0;
        String raw = value.toString();
        if (!raw.matches("[0-9]{1,19}")) return 0;
        try { return Math.max(0, Long.parseLong(raw)); }
        catch (NumberFormatException ignored) { return 0; }
    }
    private static void put(JSONObject target, String key, Object value) {
        try { target.put(key, value); }
        catch (org.json.JSONException impossible) { throw new IllegalArgumentException("Invalid ticket data."); }
    }
}
