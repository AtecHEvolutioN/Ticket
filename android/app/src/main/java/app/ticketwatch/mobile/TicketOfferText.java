package app.ticketwatch.mobile;

import java.text.SimpleDateFormat;
import java.util.Date;
import java.util.Locale;
import java.util.TimeZone;

/** Bounded, source-labelled ticket text shared by local and Discord alerts. */
public final class TicketOfferText {
    private TicketOfferText() { }

    public static boolean hasCurrentSnapshot(Models.Watch watch) {
        if (watch == null || watch.ticketSnapshot == null) return false;
        TicketData.Snapshot snapshot = watch.ticketSnapshot;
        if (!snapshot.verified || snapshot.observedAt <= 0 || snapshot.observedAt != watch.checkedAt) return false;
        if (watch.demo) return "AVAILABLE".equals(watch.state) && "demo".equals(snapshot.source);
        if (!"web".equals(snapshot.source)) return false;
        if (GenericTicketParser.ADAPTER.equals(snapshot.adapter))
            return "AVAILABLE".equals(watch.state) || ("UNKNOWN".equals(watch.state) && snapshot.offers.isEmpty());
        return "AVAILABLE".equals(watch.state) && watch.selectorsVerified && watch.offerProfile != null
                && watch.offerProfile.verified && !Models.safe(watch.offerProfile.rowSelector).isEmpty()
                && !Models.safe(watch.offerProfile.availableSelector).isEmpty();
    }

    /** Only the snapshot associated with the current positive page check is eligible. */
    public static boolean hasCurrentOffers(Models.Watch watch) {
        return hasCurrentSnapshot(watch) && "AVAILABLE".equals(watch.state)
                && watch.ticketSnapshot.offers != null && !watch.ticketSnapshot.offers.isEmpty();
    }

    public static String offer(TicketData.Offer offer) {
        if (offer == null) return "Ticket details not reported.";
        StringBuilder text = new StringBuilder("Type: ").append(clean(offer.ticketType, 120, "not reported"));
        text.append("\nPrice: ").append(price(offer));
        text.append("\n").append(quantity(offer));
        if (!Models.safe(offer.section).isEmpty()) text.append("\nSection: ").append(clean(offer.section, 80, "not reported"));
        if (!Models.safe(offer.row).isEmpty()) text.append(" · Row: ").append(clean(offer.row, 50, "not reported"));
        return cleanLines(text.toString(), 800);
    }

    public static String price(TicketData.Offer offer) {
        String value = Models.safe(offer.price);
        String currency = clean(offer.currency, 3, "");
        if (!value.matches("[0-9]{1,15}(?:\\.[0-9]{1,6})?") || !currency.matches("[A-Z]{3}")) {
            value = clean(offer.priceText, 120, "not reported");
            if (!"not reported".equals(value)) value += " (page text; amount unverified)";
        } else value += " " + currency;
        String scope = "per_ticket".equals(offer.priceScope) ? "per ticket"
                : "total".equals(offer.priceScope) ? "total price" : "price basis not reported";
        String fees = "included".equals(offer.fees) ? "fees included"
                : "excluded".equals(offer.fees) ? "fees excluded" : "fees not reported";
        return value + " · " + scope + " · " + fees;
    }

    public static String quantity(TicketData.Offer offer) {
        if (offer.quantity == null || offer.quantity < 0) return "Quantity: not reported";
        if ("listing".equals(offer.quantityKind)) return "Listing quantity: " + offer.quantity;
        if ("selectable".equals(offer.quantityKind)) return "Selectable quantity: " + offer.quantity + " (not remaining stock)";
        if ("remaining".equals(offer.quantityKind)) return "Reported remaining for this offer: " + offer.quantity;
        return "Quantity: " + offer.quantity + " (meaning not verified; not a stock count)";
    }

    public static String observation(Models.Watch watch) {
        if (!hasCurrentOffers(watch)) return "Ticket prices, types and quantities were not verified by this check.";
        SimpleDateFormat format = new SimpleDateFormat("yyyy-MM-dd HH:mm:ss 'UTC'", Locale.ROOT);
        format.setTimeZone(TimeZone.getTimeZone("UTC"));
        return (watch.demo ? "Synthetic demo" : "Event page") + " · observed "
                + format.format(new Date(watch.ticketSnapshot.observedAt))
                + "\nTotal remaining for the event: not reported. Offer quantities are not added together.";
    }

    public static String notificationDetail(Models.Watch watch) {
        String intro = watch != null && watch.demo
                ? "Synthetic availability changed. These are demo offers, not live tickets."
                : "Your verified page signal changed to available. Open the event to confirm.";
        if (!hasCurrentOffers(watch)) return intro + "\nTicket prices, types and quantities: not verified.";
        StringBuilder text = new StringBuilder(intro).append("\n\n").append(observation(watch));
        int shown = Math.min(3, watch.ticketSnapshot.offers.size());
        for (int i = 0; i < shown; i++) text.append("\n\n").append(offer(watch.ticketSnapshot.offers.get(i)));
        if (watch.ticketSnapshot.offers.size() > shown) text.append("\n\nMore observed offers are shown in the app.");
        return cleanLines(text.toString(), 3000);
    }

    static String clean(String raw, int maximum, String fallback) {
        String value = Models.safe(raw).replaceAll("[\\p{Cntrl}\\s]+", " ").trim();
        if (value.isEmpty()) return fallback;
        return clip(value, maximum);
    }

    private static String cleanLines(String value, int maximum) { return clip(value, maximum); }

    private static String clip(String value, int maximum) {
        if (value.length() <= maximum) return value;
        int end = Math.max(0, maximum - 1);
        if (end > 0 && Character.isHighSurrogate(value.charAt(end - 1))) end--;
        return value.substring(0, end) + "…";
    }
}
