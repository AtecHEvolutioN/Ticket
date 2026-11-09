package app.ticketwatch.mobile;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.LinearLayout;
import org.json.JSONArray;
import org.json.JSONObject;
import org.json.JSONTokener;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Minimal synthetic fixtures preserve regional DOM shapes and observed translation labels. */
public final class EuropeTicketChecks {
    private int assertions;
    private void check(boolean condition, String label) {
        assertions++;
        if (!condition) throw new AssertionError(label);
    }

    private static JSONObject payload(String price) throws Exception {
        return new JSONObject().put("valid", true).put("adapter", GenericTicketParser.ADAPTER)
                .put("url", "https://www.ticketmaster.se/event/synthetic-night/51401601")
                .put("rows", new JSONArray()).put("categories", new JSONArray().put(
                        new JSONObject().put("type", "Synthetic admission").put("price", price).put("selectableQuantity", 6)));
    }

    private static TicketData.Snapshot parse(JSONObject payload) {
        return GenericTicketParser.parse(payload, true, "UNKNOWN", 1234);
    }

    private static JSONObject hint(String eventUrl, String offerUrl, String code) throws Exception {
        return new JSONObject().put("eventUrl", eventUrl).put("offerUrl", offerUrl).put("currency", code);
    }

    public static int run() throws Exception {
        EuropeTicketChecks checks = new EuropeTicketChecks();
        String[][] prices = {
                {"1 525,50 Kč za kus", "1525.5", "1525.5", "CZK"},
                {"1 490,00 Kč–12 990,00 Kč za kus", "1490", "12990", "CZK"},
                {"147,30 € cada una", "147.3", "147.3", "EUR"},
                {"€ 40,34–€ 55,50 per stuk", "40.34", "55.5", "EUR"},
                {"je 27,15 €", "27.15", "27.15", "EUR"},
                {"£40.35 each", "40.35", "40.35", "GBP"}
        };
        for (String[] price : prices) {
            TicketData.Category category = parse(payload(price[0])).categories.get(0);
            checks.check(price[1].equals(category.min) && price[2].equals(category.max) && price[3].equals(category.currency),
                    "Known localized per-ticket unit is removed for numeric parsing: " + price[0]);
            checks.check(price[0].equals(category.priceText), "Localized source price text is preserved: " + price[0]);
        }
        String[] ambiguous = {
                "27,15 € + 3,00 € fees", "27,15 € per 2 tickets", "2 × 27,15 €",
                "27,15 € each + fees", "je 27,15 € total for 2", "147,30 € cada una + 5 €",
                "525 Kč za kus plus 30 Kč"
        };
        for (String price : ambiguous) {
            TicketData.Category category = parse(payload(price)).categories.get(0);
            checks.check(category.min.isEmpty() && category.max.isEmpty() && price.equals(category.priceText),
                    "Fees and multiple-ticket phrases stay display-only: " + price);
        }
        JSONObject resale = payload("1 500,00 Kč za kus").put("rows", new JSONArray().put(
                new JSONObject().put("type", "Regular").put("section", "413").put("row", "2")
                        .put("price", "3 335,00 Kč za kus").put("quantity", "Dostupných: 2")));
        TicketData.Snapshot listing = parse(resale);
        checks.check(listing.offers.size() == 1 && "3335".equals(listing.offers.get(0).price)
                        && "CZK".equals(listing.offers.get(0).currency), "Czech resale price and currency are numeric");
        checks.check(Integer.valueOf(2).equals(listing.offers.get(0).quantity)
                        && "listing".equals(listing.offers.get(0).quantityKind), "Czech colon availability label is a listing count");
        resale.getJSONArray("rows").getJSONObject(0).put("quantity", "Dostupných: 2–4");
        checks.check(parse(resale).offers.isEmpty(), "Czech availability range is not an exact listing count");

        String url = "https://www.ticketmaster.se/event/synthetic-night/51401601";
        String wrong = "https://www.ticketmaster.se/event/different-night/51401602";
        JSONObject swedish = payload("435,00 kr per biljett");
        checks.check(parse(swedish).categories.get(0).min.isEmpty(), "Bare kr is ambiguous without matching event currency");
        swedish.put("currencyHints", new JSONArray().put(hint(url, url, "SEK")));
        TicketData.Category category = parse(swedish).categories.get(0);
        checks.check("435".equals(category.min) && "SEK".equals(category.currency), "Matching event metadata supplies SEK for a visible kr price");
        swedish.put("currencyHints", new JSONArray().put(hint(wrong, wrong, "SEK")));
        checks.check(parse(swedish).categories.get(0).min.isEmpty(), "Different-event metadata cannot disambiguate kr");
        swedish.put("currencyHints", new JSONArray().put(hint(url, wrong, "SEK")));
        checks.check(parse(swedish).categories.get(0).min.isEmpty(), "Different offer URL cannot supply currency");
        swedish.put("currencyHints", new JSONArray().put(hint(url, url, "SEK")).put(hint(url, url, "NOK")));
        checks.check(parse(swedish).categories.get(0).min.isEmpty(), "Conflicting matching currencies remain ambiguous");
        swedish.remove("currencyHints");
        swedish.put("currency", "SEK");
        checks.check(parse(swedish).categories.get(0).min.isEmpty(), "An unbound top-level currency cannot disambiguate kr");
        JSONObject contextual = payload("£40.35 each");
        JSONObject first = contextual.getJSONArray("categories").getJSONObject(0)
                .put("type", "Full Price Ticket").put("section", "A").put("row", "5");
        JSONObject second = new JSONObject(first.toString()).put("price", "£50.75 each").put("section", "B").put("row", "6");
        contextual.getJSONArray("categories").put(second);
        TicketData.Snapshot locations = parse(contextual);
        checks.check(locations.verified && locations.categories.size() == 2,
                "The same ticket type at different locations retains separate categories");
        checks.check("A".equals(locations.categories.get(0).section) && "5".equals(locations.categories.get(0).row)
                        && "40.35".equals(locations.categories.get(0).min) && "B".equals(locations.categories.get(1).section)
                        && "6".equals(locations.categories.get(1).row) && "50.75".equals(locations.categories.get(1).min),
                "Category location and price stay associated with their individual rows");
        TicketData.Snapshot restored = TicketData.snapshotFromJson(TicketData.snapshotToJson(locations));
        checks.check(restored.categories.size() == 2 && "A".equals(restored.categories.get(0).section)
                        && "5".equals(restored.categories.get(0).row) && "B".equals(restored.categories.get(1).section)
                        && "6".equals(restored.categories.get(1).row), "Category location survives snapshot serialization");
        contextual.getJSONArray("categories").put(new JSONObject(first.toString()));
        checks.check(parse(contextual).categories.size() == 2, "An identical category at the same location is deduplicated");
        contextual.getJSONArray("categories").put(new JSONObject(first.toString()).put("price", "£55.25 each"));
        TicketData.Snapshot conflict = parse(contextual);
        checks.check(!conflict.verified && conflict.categories.isEmpty() && conflict.offers.isEmpty(),
                "Conflicting category prices at the same location fail closed");
        return checks.assertions;
    }

    private static String primary(String type, String price) {
        return "<li><div data-testid='ticketTypeInfo'><div><svg aria-hidden='true'></svg><div><span>"
                + type + "<button><span>Details</span></button></span><span>" + price + "</span></div></div></div>"
                + "<div data-testid='quantityStepper'><button disabled>Minus</button>"
                + "<div role='spinbutton' aria-valuemin='0' aria-valuenow='0' aria-valuemax='6'>0</div>"
                + "<button onclick='window.reservations++'>Plus</button></div></li>";
    }

    private static String resale(String inventory, String quantity, String sectionLabel, String rowLabel, String price) {
        return "<li role='button' onclick='window.reservations++'><div><dl><div><dt>Level</dt><dd>Upper</dd></div>"
                + "<div><dt>" + sectionLabel + "</dt><dd>413</dd></div><div><dt>" + rowLabel + "</dt><dd>2</dd></div></dl>"
                + "<div data-testid='ticketTypeInfo'><div><svg aria-hidden='true'></svg><div><span>" + inventory + "</span>"
                + "<span>" + quantity + "</span><span>Regular</span><span>" + price + "</span></div></div></div></div></li>";
    }

    private static String document(String map, String best, String contents, String metadata) {
        return "<!doctype html><html><head><meta name='viewport' content='width=device-width,initial-scale=1'>"
                + metadata + "</head><body><h1>Synthetic unrelated event</h1><script>window.reservations=0;</script>"
                + (best.isEmpty() ? "" : "<button>" + map + "</button><button aria-pressed='false' onclick=\"document.getElementById('tickets').style.display='block';this.setAttribute('aria-pressed','true')\">" + best + "</button>")
                + "<div id='tickets'" + (best.isEmpty() ? "" : " style='display:none'") + "><ul>" + contents + "</ul>"
                + "<button data-testid='findTicketsBtn' onclick='window.reservations++'>Continue</button></div></body></html>";
    }

    private static JSONObject evaluate(Instrumentation instrumentation, WebView web, String script) throws Exception {
        CountDownLatch finished = new CountDownLatch(1);
        String[] value = new String[1];
        instrumentation.runOnMainSync(() -> web.evaluateJavascript(script, result -> { value[0] = result; finished.countDown(); }));
        if (!finished.await(20, TimeUnit.SECONDS)) throw new AssertionError("Regional fixture evaluation timed out");
        Object parsed = new JSONTokener(value[0]).nextValue();
        return parsed instanceof String ? new JSONObject((String) parsed) : (JSONObject) parsed;
    }

    private static void load(Instrumentation instrumentation, WebView web, String url, String html) throws Exception {
        CountDownLatch loaded = new CountDownLatch(1);
        instrumentation.runOnMainSync(() -> {
            web.setWebViewClient(new WebViewClient() {
                @Override public void onPageFinished(WebView view, String location) { loaded.countDown(); }
            });
            web.loadDataWithBaseURL(url, html, "text/html", "UTF-8", null);
        });
        if (!loaded.await(30, TimeUnit.SECONDS)) throw new AssertionError("Regional fixture failed to load");
        instrumentation.waitForIdleSync();
    }

    public static int browser(Instrumentation instrumentation) throws Exception {
        EuropeTicketChecks checks = new EuropeTicketChecks();
        Activity activity = instrumentation.startActivitySync(new Intent(instrumentation.getTargetContext(), MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        WebView[] web = new WebView[1];
        try {
            instrumentation.runOnMainSync(() -> {
                web[0] = new WebView(activity);
                web[0].getSettings().setJavaScriptEnabled(true);
                web[0].getSettings().setBlockNetworkLoads(true);
                activity.setContentView(web[0], new LinearLayout.LayoutParams(-1, -1));
            });
            String[][] markets = {
                    {"cz", "Mapa míst", "Nejlepší dostupné", "Základní", "1 490,00 Kč–12 990,00 Kč za kus", "1490", "12990", "CZK",
                            "Ověřená přeprodávaná vstupenka", "Dostupných: 2", "Sekce", "Řada", "3 335,00 Kč za kus", "3335"},
                    {"es", "Ver mapa de asientos", "Ver lo mejor disponible", "Asiento reservado", "45,30 €–79,30 € cada una", "45.3", "79.3", "EUR",
                            "", "", "", "", "", ""},
                    {"nl", "Bekijk plattegrond", "Best beschikbaar", "Reguliere verkoop", "€ 40,34–€ 55,50 per stuk", "40.34", "55.5", "EUR",
                            "Verified Resale Ticket", "2 beschikbaar", "Vak", "Rij", "€ 74,60 per stuk", "74.6"}
            };
            for (String[] market : markets) {
                String rows = primary(market[3], market[4]);
                if (!market[8].isEmpty()) rows += resale(market[8], market[9], market[10], market[11], market[12]);
                String url = "https://www.ticketmaster." + market[0] + "/event/synthetic-regional-event/67892104";
                load(instrumentation, web[0], url, document(market[1], market[2], rows, ""));
                JSONObject preparation = evaluate(instrumentation, web[0], GenericTicketParser.prepareScript());
                checks.check(preparation.optBoolean("clicked"), "Observed localized best-available control opens: " + market[0]);
                JSONObject extracted = evaluate(instrumentation, web[0], GenericTicketParser.extractionScript());
                TicketData.Snapshot snapshot = parse(extracted);
                checks.check(snapshot.categories.size() == 1 && market[5].equals(snapshot.categories.get(0).min)
                                && market[6].equals(snapshot.categories.get(0).max) && market[7].equals(snapshot.categories.get(0).currency),
                        "Regional DOM preserves numeric range and currency: " + market[0]);
                checks.check(market[4].equals(snapshot.categories.get(0).priceText), "Regional DOM preserves source unit wording: " + market[0]);
                if (market[8].isEmpty()) {
                    checks.check(snapshot.offers.isEmpty(), "Primary categories alone do not establish available listings");
                } else {
                    checks.check(snapshot.offers.size() == 1 && market[13].equals(snapshot.offers.get(0).price)
                                    && Integer.valueOf(2).equals(snapshot.offers.get(0).quantity), "Localized resale label and count are recognized: " + market[0]);
                    checks.check("413".equals(snapshot.offers.get(0).section) && "2".equals(snapshot.offers.get(0).row),
                            "Localized section and row labels are recognized: " + market[0]);
                }
                checks.check(evaluate(instrumentation, web[0], "({reservations:window.reservations})").optInt("reservations") == 0,
                        "Regional preparation never selects quantity or reserves: " + market[0]);
            }
            String swedishUrl = "https://www.ticketmaster.se/event/synthetic-night/51401601";
            String primary = primary("Ståplats", "435,00 kr per biljett");
            for (boolean sameEvent : new boolean[]{true, false}) {
                String eventUrl = sameEvent ? swedishUrl : "https://www.ticketmaster.se/event/different-night/51401602";
                String metadata = "<script type='application/ld+json'>{\"@context\":\"https://schema.org\",\"@type\":\"MusicEvent\",\"url\":\""
                        + eventUrl + "\",\"offers\":{\"@type\":\"Offer\",\"url\":\"" + eventUrl + "\",\"priceCurrency\":\"SEK\"}}</script>";
                load(instrumentation, web[0], swedishUrl, document("", "", primary, metadata));
                TicketData.Snapshot snapshot = parse(evaluate(instrumentation, web[0], GenericTicketParser.extractionScript()));
                TicketData.Category category = snapshot.categories.get(0);
                checks.check(sameEvent ? "435".equals(category.min) && "SEK".equals(category.currency) : category.min.isEmpty(),
                        "Swedish kr uses currency only from matching Event metadata");
                checks.check(snapshot.offers.isEmpty() && "435,00 kr per biljett".equals(category.priceText),
                        "Swedish displayed category remains separate from stock with raw wording intact");
            }
            String malformedMetadata = "<script type='application/ld+json'>{\"@type\":\"MusicEvent\",\"url\":\""
                    + swedishUrl + "\",\"offers\":{\"url\":{\"@id\":\"https://www.ticketmaster.se/event/different-night/51401602\"},"
                    + "\"priceCurrency\":\"SEK\"}}</script>";
            load(instrumentation, web[0], swedishUrl, document("", "", primary, malformedMetadata));
            TicketData.Snapshot malformed = parse(evaluate(instrumentation, web[0], GenericTicketParser.extractionScript()));
            checks.check(malformed.categories.get(0).min.isEmpty(),
                    "A present non-string offer URL cannot be treated as an absent currency binding");
            String quickpick = "<div role='button' onclick='window.reservations++'><div><dl><div><dt>Section</dt>"
                    + "<dd>A</dd></div><div><dt>Row</dt><dd>5</dd></div></dl><div><div data-testid='ticketTypeInfo'><div><svg aria-hidden='true'></svg>"
                    + "<div><span>Full Price Ticket</span><span>£40.35 each</span></div></div></div></div></div></div>";
            String secondQuickpick = quickpick.replace("<dd>A</dd>", "<dd>B</dd>").replace("<dd>5</dd>", "<dd>6</dd>")
                    .replace("£40.35 each", "£50.75 each");
            String quickpickHtml = "<!doctype html><html><body><h1>Synthetic venue fixture</h1>"
                    + "<script>window.reservations=0;</script><button>2 Tickets</button><div data-testid='quickpicksList'>"
                    + quickpick + secondQuickpick + quickpick.replace("role='button'", "role='button' style='display:none'")
                    + quickpick.replace("role='button'", "role='button' aria-disabled='true'") + "</div></body></html>";
            load(instrumentation, web[0], "https://www.ticketmaster.co.uk/event/synthetic-event/3E00654CFB931234", quickpickHtml);
            TicketData.Snapshot quickpicks = parse(evaluate(instrumentation, web[0], GenericTicketParser.extractionScript()));
            checks.check(quickpicks.categories.size() == 2 && quickpicks.offers.isEmpty(),
                    "Visible enabled quickpicks retain distinct locations without claiming stock");
            TicketData.Category quick = quickpicks.categories.get(0);
            checks.check("Full Price Ticket".equals(quick.ticketType) && "40.35".equals(quick.min)
                            && "GBP".equals(quick.currency) && "£40.35 each".equals(quick.priceText),
                    "Quickpicks preserve type and each-price while parsing the numeric value");
            checks.check(quick.selectableQuantity == null,
                    "Quickpicks do not infer a listing count or capacity from toolbar ticket quantity");
            checks.check("A".equals(quick.section) && "5".equals(quick.row)
                            && "B".equals(quickpicks.categories.get(1).section) && "6".equals(quickpicks.categories.get(1).row)
                            && "50.75".equals(quickpicks.categories.get(1).min),
                    "Quickpicks DOM binds each same-type price to its section and row");
            TicketData.Snapshot storedQuickpicks = TicketData.snapshotFromJson(TicketData.snapshotToJson(quickpicks));
            checks.check(storedQuickpicks.categories.size() == 2 && "A".equals(storedQuickpicks.categories.get(0).section)
                            && "6".equals(storedQuickpicks.categories.get(1).row),
                    "Extracted quickpicks preserve location after snapshot serialization");
            checks.check(evaluate(instrumentation, web[0], "({reservations:window.reservations})").optInt("reservations") == 0,
                    "Quickpicks extraction never selects the purchasable row");
            load(instrumentation, web[0], "https://www.ticketmaster.co.uk/event/synthetic-event/3E00654CFB931234",
                    quickpickHtml.replace("<dd>B</dd>", "<dd>A</dd>").replace("<dd>6</dd>", "<dd>5</dd>"));
            TicketData.Snapshot conflictingQuickpicks = parse(evaluate(instrumentation, web[0], GenericTicketParser.extractionScript()));
            checks.check(!conflictingQuickpicks.verified && conflictingQuickpicks.categories.isEmpty() && conflictingQuickpicks.offers.isEmpty(),
                    "Quickpicks with conflicting prices for the same context cannot produce a verified snapshot");
        } finally {
            instrumentation.runOnMainSync(() -> { if (web[0] != null) web[0].destroy(); activity.finish(); });
            instrumentation.waitForIdleSync();
        }
        return checks.assertions;
    }
}
