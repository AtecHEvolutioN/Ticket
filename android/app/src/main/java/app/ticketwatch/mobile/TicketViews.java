package app.ticketwatch.mobile;

import android.app.Activity;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.view.View;
import android.widget.LinearLayout;
import android.widget.TextView;

import java.math.BigDecimal;
import java.text.DateFormat;
import java.util.Date;
import java.util.List;
import java.util.Currency;

/** Shared ticket presentation. Catalog prices never become evidence of live stock. */
public final class TicketViews {
    private static final int FIELD = Color.rgb(10, 23, 38);
    private static final int LINE = Color.rgb(38, 57, 76);
    private static final int TEXT = Color.rgb(237, 244, 250);
    private static final int MUTED = Color.rgb(153, 174, 192);
    private static final int TEAL = Color.rgb(89, 229, 202);
    private static final int AMBER = Color.rgb(255, 203, 118);

    private TicketViews() { }

    public static void addCatalog(Activity activity, LinearLayout parent, List<TicketData.PriceRange> ranges) {
        LinearLayout block = block(activity, parent);
        block.setTag("ticket_catalog");
        add(activity, block, "ADVERTISED PRICE RANGE", 10, MUTED, true, 0);
        int added = 0;
        if (ranges != null) {
            for (TicketData.PriceRange range : ranges) {
                if (range == null || added >= 8) continue;
                String min = amount(range.min, range.currency);
                String max = amount(range.max, range.currency);
                if (min.isEmpty() && max.isEmpty()) continue;
                String amount = !min.isEmpty() && !max.isEmpty() ? (equalAmounts(min, max) ? min : min + " – " + max)
                        : min.isEmpty() ? "Up to " + max : "From " + min;
                String currency = clean(range.currency);
                if (!currency.isEmpty()) amount += " " + currency;
                else amount += " · currency not reported";
                add(activity, block, amount, 16, TEXT, true, 6);
                added++;
            }
        }
        if (added == 0) add(activity, block, "Not reported", 13, MUTED, false, 6);
        else add(activity, block, "Discovery catalog · current offers and fees may differ.", 11, MUTED, false, 6);
    }

    public static void addSnapshot(Activity activity, LinearLayout parent, Models.Watch watch, boolean compact) {
        LinearLayout block = block(activity, parent);
        block.setTag("ticket_snapshot");
        TicketData.Snapshot snapshot = watch == null ? null : watch.ticketSnapshot;
        boolean demo = watch != null && watch.demo;
        boolean current = TicketOfferText.hasCurrentOffers(watch);
        add(activity, block, demo ? "DEMO TICKET OFFERS" : "OBSERVED TICKET OFFERS", 10, demo ? AMBER : TEAL, true, 0);
        int count = current && snapshot.offers != null ? snapshot.offers.size() : 0;
        if (TicketOfferText.hasCurrentSnapshot(watch) && !snapshot.categories.isEmpty()) {
            add(activity, block, "DISPLAYED TICKET CATEGORIES", 10, MUTED, true, 12);
            int categoriesShown = Math.min(snapshot.categories.size(), compact ? 2 : 50);
            for (int i=0;i<categoriesShown;i++) {
                TicketData.Category c=snapshot.categories.get(i);
                add(activity, block, c.ticketType, 14, TEXT, true, 10);
                add(activity, block, c.priceText, 14, TEXT, false, 4);
                if (!c.section.isEmpty() || !c.row.isEmpty())
                    add(activity, block, (c.section.isEmpty() ? "" : "Section " + c.section)
                            + (c.row.isEmpty() ? "" : (c.section.isEmpty() ? "" : " · ") + "Row " + c.row), 11, MUTED, false, 4);
                if (c.selectableQuantity != null && !compact)
                    add(activity, block, "Selectable maximum: " + c.selectableQuantity + " · remaining stock unknown", 11, MUTED, false, 4);
            }
            add(activity, block, "Displayed prices may be ranges. Categories do not confirm stock for a requested quantity.", 11, MUTED, false, 8);
        }
        if (count == 0) {
            add(activity, block, "Available ticket listings: not reported", 13, MUTED, false, 8);
            if (!compact) add(activity, block, demo ? "Simulate available in the Watchlist to preview synthetic offers."
                    : "These details appear after a verified page check can read individual ticket offers.", 12, MUTED, false, 7);
            add(activity, block, "Total remaining: not reported", 11, MUTED, false, 8);
            return;
        }
        int limit = Math.min(count, compact ? 2 : 50);
        for (int i = 0; i < limit; i++) {
            TicketData.Offer offer = snapshot.offers.get(i);
            if (offer == null) continue;
            LinearLayout row = new LinearLayout(activity);
            row.setOrientation(LinearLayout.VERTICAL);
            row.setTag("ticket_offer_" + i);
            if (i > 0) {
                View divider = new View(activity);
                divider.setBackgroundColor(LINE);
                LinearLayout.LayoutParams dividerParams = new LinearLayout.LayoutParams(-1, dp(activity, 1));
                dividerParams.topMargin = dp(activity, compact ? 12 : 16);
                block.addView(divider, dividerParams);
            }
            LinearLayout.LayoutParams rowParams = new LinearLayout.LayoutParams(-1, -2);
            rowParams.topMargin = dp(activity, 11);
            block.addView(row, rowParams);
            add(activity, row, fallback(offer.ticketType, "Ticket type not reported"), compact ? 14 : 17, TEXT, true, 0);
            add(activity, row, price(offer), compact ? 15 : 20, TEAL, true, 6);
            add(activity, row, priceScope(offer.priceScope) + " · " + fees(offer.fees), 11, MUTED, false, 3);
            add(activity, row, quantity(offer), 13, TEXT, false, 7);
            if (!compact) {
                String seat = clean(offer.section).isEmpty() ? "Section not reported" : "Section " + clean(offer.section);
                if (!clean(offer.row).isEmpty()) seat += " · Row " + clean(offer.row);
                add(activity, row, seat, 12, MUTED, false, 6);
            }
        }
        if (count > limit) add(activity, block, compact ? "+" + (count - limit) + " more observed offers · open ticket details"
                : "Showing the first " + limit + " of " + count + " observed offers", 12, TEAL, false, 10);
        add(activity, block, "Total remaining: not reported", 11, MUTED, false, 13);
        if (!compact) add(activity, block, "Each quantity applies to its offer. Listings can overlap and are not added together.", 11, MUTED, false, 5);
        add(activity, block, (demo ? "Synthetic demo" : "Public page observation") + " · "
                + DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(new Date(snapshot.observedAt)),
                10, MUTED, false, 8);
        if (demo && !compact) add(activity, block, "Demo values only; no live tickets were found or reserved.", 11, AMBER, false, 5);
    }

    static String price(TicketData.Offer offer) {
        String amount = amount(offer.price, offer.currency);
        String currency = clean(offer.currency);
        if (!amount.isEmpty()) return amount + (currency.isEmpty() ? " · currency not reported" : " " + currency);
        String raw = clean(offer.priceText);
        return raw.isEmpty() ? "Price not reported" : "Price shown: " + raw;
    }

    static String quantity(TicketData.Offer offer) {
        if (offer.quantity == null || offer.quantity < 0) {
            String raw = clean(offer.quantityText);
            return raw.isEmpty() ? "Quantity not reported" : "Quantity shown: " + raw + " · meaning unverified";
        }
        String number = String.valueOf(offer.quantity);
        String noun = offer.quantity == 1 ? " ticket" : " tickets";
        if ("listing".equals(offer.quantityKind)) return number + noun + " in this listing";
        if ("selectable".equals(offer.quantityKind)) return "Selectable quantity: " + number + " · not remaining stock";
        if ("remaining".equals(offer.quantityKind)) return "Reported remaining for this offer: " + number;
        return "Quantity shown: " + number + " · meaning unverified";
    }

    static String priceScope(String scope) {
        return "per_ticket".equals(scope) ? "Per ticket" : "total".equals(scope) ? "Total offer price" : "Price basis not reported";
    }

    static String fees(String scope) {
        return "included".equals(scope) ? "fees included" : "excluded".equals(scope) ? "fees extra" : "fee inclusion unknown";
    }

    private static boolean equalAmounts(String first, String second) {
        try { return new BigDecimal(first).compareTo(new BigDecimal(second)) == 0; }
        catch (Exception ignored) { return first.equals(second); }
    }

    private static String decimal(String value) {
        String result = clean(value);
        if (!result.matches("[0-9]{1,15}(\\.[0-9]{1,6})?")) return "";
        return result;
    }

    private static String amount(String value, String currency) {
        String canonical = decimal(value);
        if (canonical.isEmpty()) return "";
        try {
            BigDecimal number = new BigDecimal(canonical);
            int digits = Currency.getInstance(clean(currency)).getDefaultFractionDigits();
            if (digits >= 0 && digits <= 6) return number.setScale(Math.max(number.scale(), digits)).toPlainString();
        } catch (IllegalArgumentException ignored) { }
        return canonical;
    }

    private static String clean(String value) {
        String result = value == null ? "" : value.trim().replaceAll("[\\r\\n\\t]+", " ");
        return result.length() > 240 ? result.substring(0, 240) : result;
    }

    private static String fallback(String value, String fallback) {
        String result = clean(value);
        return result.isEmpty() ? fallback : result;
    }

    private static LinearLayout block(Activity activity, LinearLayout parent) {
        LinearLayout block = new LinearLayout(activity);
        block.setOrientation(LinearLayout.VERTICAL);
        block.setPadding(dp(activity, 13), dp(activity, 13), dp(activity, 13), dp(activity, 13));
        GradientDrawable shape = new GradientDrawable();
        shape.setColor(FIELD);
        shape.setCornerRadius(dp(activity, 12));
        shape.setStroke(dp(activity, 1), LINE);
        block.setBackground(shape);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
        params.topMargin = dp(activity, 12);
        parent.addView(block, params);
        return block;
    }

    private static void add(Activity activity, LinearLayout parent, String value, int size, int color, boolean bold, int top) {
        TextView text = new TextView(activity);
        text.setText(value);
        text.setTextSize(size);
        text.setTextColor(color);
        text.setIncludeFontPadding(false);
        text.setLineSpacing(dp(activity, 2), 1);
        if (bold) text.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
        params.topMargin = dp(activity, top);
        parent.addView(text, params);
    }

    private static int dp(Activity activity, int value) {
        return Math.round(value * activity.getResources().getDisplayMetrics().density);
    }
}
