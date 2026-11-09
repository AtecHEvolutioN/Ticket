package app.ticketwatch.mobile;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Context;
import android.content.Intent;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.LinearLayout;
import org.json.JSONArray;
import org.json.JSONObject;
import org.json.JSONTokener;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Ticket details regression coverage. Every offer and HTML page here is synthetic. */
public final class TicketChecks {
    private int count;
    private void check(boolean value, String message) {
        count++;
        if (!value) throw new AssertionError(message);
    }

    public static int run(Context context) throws Exception {
        TicketChecks checks = new TicketChecks();
        checks.catalog();
        checks.parser();
        checks.persistence(context);
        checks.alertPayload();
        return checks.count;
    }

    private void catalog() throws Exception {
        List<TicketData.PriceRange> ranges = TicketData.readPriceRanges(new JSONArray(
                "[{\"type\":\"standard\",\"currency\":\"PLN\",\"min\":435.85,\"max\":910.80},"
                + "{\"currency\":\"JOD\",\"min\":1.234,\"max\":2.345}]"));
        check(ranges.size() == 2, "Catalog retains separate currency ranges");
        check("435.85".equals(ranges.get(0).min), "Catalog money retains exact decimal value");
        check("1.234".equals(ranges.get(1).min), "Catalog does not force all currencies to two decimals");
        check(TicketData.readPriceRanges(null).isEmpty(), "Missing price range remains unknown");
        check(TicketData.readPriceRanges(new JSONArray("[{\"currency\":\"PLN\",\"min\":-1,\"max\":10}]")).isEmpty(), "Negative catalog price rejected");
        check(TicketData.readPriceRanges(new JSONArray("[{\"currency\":\"PLN\",\"min\":20,\"max\":10}]")).isEmpty(), "Reversed catalog range rejected");
        check(TicketData.readPriceRanges(new JSONArray("[{\"currency\":\"nonsense\",\"min\":1,\"max\":10}]")).isEmpty(), "Unknown currency cannot become a numeric quote");
        Models.Event event = new Models.Event();
        event.name = "Synthetic catalog fixture";
        event.url = "https://www.ticketmaster.pl/event/offline/000001";
        event.priceRanges = ranges;
        Models.Watch watch = Models.Watch.fromEvent(event, "Synthetic artist");
        check(watch.priceRanges.size() == 2, "Saved event retains advertised prices");
        check(watch.ticketSnapshot.offers.isEmpty() && "UNKNOWN".equals(watch.state), "Catalog prices never create available offers");
    }

    private static TicketData.Profile profile() {
        TicketData.Profile profile = new TicketData.Profile();
        profile.rowSelector = ".offer";
        profile.availableSelector = ".buy";
        profile.typeSelector = ".type";
        profile.sectionSelector = ".section";
        profile.priceSelector = ".price";
        profile.quantitySelector = ".quantity";
        profile.currency = "PLN";
        profile.decimalSeparator = ",";
        profile.quantityKind = "listing";
        profile.priceScope = "per_ticket";
        profile.fees = "unknown";
        profile.verified = true;
        return profile;
    }

    private static JSONObject row() throws Exception {
        return new JSONObject().put("type", "Standard admission").put("section", "A1")
                .put("price", "435,85 zł").put("quantity", "2").put("quantitySelectable", false);
    }

    private static JSONObject payload(JSONObject row) throws Exception {
        return new JSONObject().put("valid", true).put("truncated", false).put("rows", new JSONArray().put(row));
    }

    private static TicketData.Snapshot sample() throws Exception {
        return TicketPageParser.parse(payload(row()), profile(), true, "AVAILABLE", System.currentTimeMillis());
    }

    private void parser() throws Exception {
        check("1234.5".equals(TicketPageParser.parsePrice("1 234,50 zł", ",", "PLN")), "Comma currency price normalizes exactly");
        check("1234.5".equals(TicketPageParser.parsePrice("1,234.50 USD", ".", "USD")), "Dot currency price normalizes exactly");
        check(TicketPageParser.parsePrice("435–910 PLN", ".", "PLN").isEmpty(), "Price range cannot masquerade as one offer price");
        check(TicketPageParser.parsePrice("-5 PLN", ".", "PLN").isEmpty(), "Negative offer price rejected");
        check(Integer.valueOf(2).equals(TicketPageParser.parseQuantity("2")), "Exact quantity parsed");
        check(TicketPageParser.parseQuantity("1–4") == null, "Quantity range remains unknown");
        check(TicketPageParser.parseQuantity("2.5") == null, "Fractional ticket count rejected");
        TicketData.Snapshot snapshot = sample();
        check(snapshot.verified && snapshot.offers.size() == 1, "Verified visible offer accepted");
        check("Standard admission".equals(snapshot.offers.get(0).ticketType), "Observed ticket type retained");
        check("435.85".equals(snapshot.offers.get(0).price) && "PLN".equals(snapshot.offers.get(0).currency), "Offer amount and currency retained");
        check("listing".equals(snapshot.offers.get(0).quantityKind), "Listing quantity is not global remaining stock");
        check("unknown".equals(snapshot.offers.get(0).fees), "Unknown fee scope stays unknown");
        check(TicketPageParser.parse(payload(row()), profile(), false, "AVAILABLE", 1).offers.isEmpty(), "Wrong event cannot provide offers");
        check(TicketPageParser.parse(payload(row()), profile(), true, "BLOCKED", 1).offers.isEmpty(), "Blocked page cannot provide offers");
        TicketData.Profile unverified = profile();
        unverified.verified = false;
        check(TicketPageParser.parse(payload(row()), unverified, true, "AVAILABLE", 1).offers.isEmpty(), "Unverified selectors cannot provide offers");
        check(TicketPageParser.parse(payload(row()).put("valid", false), profile(), true, "AVAILABLE", 1).offers.isEmpty(), "Invalid selector extraction remains unknown");
        check(TicketPageParser.parse(payload(row().put("quantity", "0")), profile(), true, "AVAILABLE", 1).offers.isEmpty(), "Zero-quantity row is not an available offer");
        TicketData.Profile remaining = profile();
        remaining.quantityKind = "remaining";
        TicketData.Snapshot dropdown = TicketPageParser.parse(payload(row().put("quantitySelectable", true)), remaining, true, "AVAILABLE", 1);
        check("selectable".equals(dropdown.offers.get(0).quantityKind), "Dropdown cannot be reported as remaining stock");
        JSONObject duplicate = payload(row());
        duplicate.getJSONArray("rows").put(row());
        check(TicketPageParser.parse(duplicate, profile(), true, "AVAILABLE", 1).offers.size() == 1, "Duplicate offer rows do not inflate offers");
        TicketData.Profile conflictingCurrency = profile();
        conflictingCurrency.currency = "EUR";
        TicketData.Offer conflict = TicketPageParser.parse(payload(row().put("price", "$435.85")), conflictingCurrency,
                true, "AVAILABLE", 1).offers.get(0);
        check(conflict.currency.isEmpty() && conflict.price.isEmpty(), "Custom EUR profile cannot label a dollar price EUR");
        conflict = TicketPageParser.parse(payload(row().put("price", "435,85 zł")), conflictingCurrency,
                true, "AVAILABLE", 1).offers.get(0);
        check(conflict.currency.isEmpty() && conflict.price.isEmpty(), "Custom currency must agree with an explicit price symbol");
        JSONObject missing = row().put("quantity", "").put("price", "");
        TicketData.Offer incomplete = TicketPageParser.parse(payload(missing), profile(), true, "AVAILABLE", 1).offers.get(0);
        check(incomplete.quantity == null && incomplete.price.isEmpty(), "Missing offer fields remain unknown, not zero");
    }

    private static Models.Watch watch() {
        Models.Event event = new Models.Event();
        event.name = "Synthetic ticket details fixture";
        event.url = "https://www.ticketmaster.pl/event/offline/000002";
        Models.Watch watch = Models.Watch.fromEvent(event, "Synthetic artist");
        watch.availableSelector = ".ready";
        watch.unavailableSelector = ".soldout";
        watch.selectorsVerified = true;
        watch.offerProfile = profile();
        return watch;
    }

    private void persistence(Context context) throws Exception {
        WatchStore store = new WatchStore(context);
        Models.Watch watch = watch();
        store.remove(watch.id);
        try {
            store.save(watch);
            check(!store.observe(watch.id, "AVAILABLE", "Synthetic baseline", sample()), "Offer-bearing initial observation remains quiet");
            Models.Watch saved = new WatchStore(context).get(watch.id);
            check(saved.ticketSnapshot.offers.size() == 1 && Integer.valueOf(2).equals(saved.ticketSnapshot.offers.get(0).quantity), "Offer details survive reopening storage");
            saved.name = "Renamed synthetic fixture";
            store.save(saved);
            check(store.get(watch.id).ticketSnapshot.offers.size() == 1, "Metadata edit preserves current offer evidence");
            store.observe(watch.id, "BLOCKED", "Synthetic blocked response");
            check(store.get(watch.id).ticketSnapshot.offers.isEmpty(), "Blocked response removes offers from the current display");
            check("AVAILABLE".equals(store.get(watch.id).lastDefinite), "Blocked response still preserves deduplication baseline");
            check(!store.observe(watch.id, "AVAILABLE", "Recovery", sample()), "Offer recovery does not invent a restock");
            saved = store.get(watch.id);
            saved.offerProfile.priceSelector = ".new-price";
            store.save(saved);
            check(store.get(watch.id).ticketSnapshot.offers.isEmpty() && "UNKNOWN".equals(store.get(watch.id).lastDefinite), "Offer-profile edit resets evidence and baseline");
            TicketData.Snapshot invalid = sample();
            invalid.verified = false;
            store.observe(watch.id, "AVAILABLE", "Unverified offer data", invalid);
            check(store.get(watch.id).ticketSnapshot.offers.isEmpty(), "Storage cannot expose unverified offers");
            TicketData.Snapshot demo = sample();
            demo.source = "demo";
            store.observe(watch.id, "AVAILABLE", "Wrong source", demo);
            check(store.get(watch.id).ticketSnapshot.offers.isEmpty(), "Demo offers cannot populate a real watch");
        } finally { store.remove(watch.id); }
    }

    private void alertPayload() throws Exception {
        Models.Watch watch = watch();
        watch.state = "AVAILABLE";
        watch.lastDefinite = "AVAILABLE";
        watch.ticketSnapshot = sample();
        watch.checkedAt = watch.ticketSnapshot.observedAt;
        JSONObject payload = DiscordClient.buildAlertPayload("", watch);
        String text = payload.toString();
        check(text.contains("435.85") && text.contains("PLN") && text.contains("Standard admission"), "Discord payload includes observed offer price and type");
        check(text.toLowerCase(java.util.Locale.ROOT).contains("listing"), "Discord quantity includes its meaning");
        check(payload.getJSONObject("allowed_mentions").getJSONArray("parse").length() == 0, "Offer text cannot trigger broad Discord mentions");
        watch.ticketSnapshot.observedAt--;
        check(!TicketOfferText.hasCurrentOffers(watch), "Earlier offer snapshot cannot appear in a current alert");
        watch.ticketSnapshot.observedAt = watch.checkedAt;
        watch.ticketSnapshot.source = "demo";
        check(!TicketOfferText.hasCurrentOffers(watch), "Synthetic data cannot appear in a real ticket alert");
        watch.demo = true;
        boolean demoRejected = false;
        try { DiscordClient.buildAlertPayload("", watch); }
        catch (Exception expected) { demoRejected = true; }
        check(demoRejected, "Demo cannot generate an external Discord payload");
        watch.demo = false;
        watch.ticketSnapshot = new TicketData.Snapshot();
        check(!DiscordClient.buildAlertPayload("", watch).toString().contains("435.85"), "Unknown offers do not reuse stale alert prices");
    }

    public static int browser(Instrumentation instrumentation) throws Exception {
        TicketChecks checks = new TicketChecks();
        Context context = instrumentation.getTargetContext();
        Activity activity = instrumentation.startActivitySync(new Intent(context, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        WebView[] web = new WebView[1];
        CountDownLatch loaded = new CountDownLatch(1);
        try {
            instrumentation.runOnMainSync(() -> {
                web[0] = new WebView(activity);
                web[0].getSettings().setJavaScriptEnabled(true);
                web[0].getSettings().setBlockNetworkLoads(true);
                web[0].setWebViewClient(new WebViewClient() {
                    @Override public void onPageFinished(WebView view, String url) { loaded.countDown(); }
                });
                activity.setContentView(web[0], new LinearLayout.LayoutParams(-1, -1));
                String visible = "<div class='offer'><span class='type'>Standing</span><span class='section'>GA</span>"
                        + "<span class='price'>435,85 zł</span><select class='quantity'><option>1</option><option>2</option>"
                        + "<option disabled>8</option></select><button class='buy'>Buy</button></div>";
                String html = "<!doctype html><html><body>" + visible + visible
                        + "<div class='offer' style='display:none'><span class='type'>Hidden</span><button class='buy'>Buy</button></div>"
                        + "<div class='offer'><span class='type'>Sold out</span><button class='buy' disabled>Buy</button></div>"
                        + "<fieldset disabled><div class='offer'><span class='type'>Disabled sale</span><button class='buy'>Buy</button></div></fieldset>"
                        + "<div class='offer'><span class='type'>No evidence</span><span class='price'>1 PLN</span></div>"
                        + "</body></html>";
                web[0].loadDataWithBaseURL("https://www.ticketmaster.pl/event/offline/000003", html, "text/html", "UTF-8", null);
            });
            checks.check(loaded.await(30, TimeUnit.SECONDS), "Offline offer fixture loads in Android WebView");
            TicketData.Profile profile = profile();
            profile.quantityKind = "remaining";
            CountDownLatch evaluated = new CountDownLatch(1);
            String[] response = new String[1];
            instrumentation.runOnMainSync(() -> web[0].evaluateJavascript(TicketPageParser.extractionScript(profile), value -> {
                response[0] = value;
                evaluated.countDown();
            }));
            checks.check(evaluated.await(20, TimeUnit.SECONDS), "Visible offer extraction completes");
            Object decoded = new JSONTokener(response[0]).nextValue();
            JSONObject extraction = decoded instanceof String ? new JSONObject((String) decoded) : (JSONObject) decoded;
            TicketData.Snapshot result = TicketPageParser.parse(extraction, profile, true, "AVAILABLE", System.currentTimeMillis());
            checks.check(result.offers.size() == 1, "DOM ignores hidden, disabled, missing-positive and duplicate offer rows");
            TicketData.Offer offer = result.offers.get(0);
            checks.check("Standing".equals(offer.ticketType) && "435.85".equals(offer.price), "WebView extracts visible ticket type and localized price");
            checks.check(Integer.valueOf(2).equals(offer.quantity) && "selectable".equals(offer.quantityKind), "WebView dropdown excludes disabled quantities and cannot claim remaining stock");
        } finally {
            instrumentation.runOnMainSync(() -> { if (web[0] != null) web[0].destroy(); activity.finish(); });
            instrumentation.waitForIdleSync();
        }
        return checks.count;
    }
}
