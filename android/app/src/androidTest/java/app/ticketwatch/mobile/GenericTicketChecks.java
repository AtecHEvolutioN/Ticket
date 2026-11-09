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
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Offline regional-layout checks. All event names, identifiers and prices are synthetic. */
public final class GenericTicketChecks {
    private int assertions;
    private void check(boolean condition, String message) {
        assertions++;
        if (!condition) throw new AssertionError(message);
    }

    private static JSONObject payload() throws Exception {
        JSONObject row = new JSONObject().put("type", "Official Platinum Ticket")
                .put("section", "B12").put("row", "15").put("price", "625,50 zł")
                .put("quantity", "Dostępne 2");
        JSONObject category = new JSONObject().put("type", "Reserved seating")
                .put("price", "190,00 zł–450,50 zł").put("selectableQuantity", 6);
        return new JSONObject().put("valid", true).put("adapter", "ticketmaster-regional")
                .put("rows", new JSONArray().put(row)).put("categories", new JSONArray().put(category));
    }

    public static int run(Context context) throws Exception {
        GenericTicketChecks checks = new GenericTicketChecks();
        checks.eventIdentities();
        JSONObject page = payload();
        TicketData.Snapshot sample = GenericTicketParser.parse(page, true, "UNKNOWN", 100);
        checks.check(sample.verified && "ticketmaster-regional".equals(sample.adapter), "Built-in adapter records its evidence source");
        checks.check(sample.offers.size() == 1 && sample.categories.size() == 1, "Primary categories stay separate from resale offers");
        TicketData.Offer offer = sample.offers.get(0);
        checks.check("625.5".equals(offer.price) && "PLN".equals(offer.currency), "Generic parser normalizes a localized per-listing price");
        checks.check(Integer.valueOf(2).equals(offer.quantity) && "listing".equals(offer.quantityKind), "Localized availability label is a listing quantity");
        checks.check("B12".equals(offer.section) && "15".equals(offer.row), "Generic parser retains section and row");
        TicketData.Category category = sample.categories.get(0);
        checks.check("190".equals(category.min) && "450.5".equals(category.max), "Primary range endpoints remain separate numeric values");
        checks.check(Integer.valueOf(6).equals(category.selectableQuantity), "Primary quantity maximum is only selectable capacity");
        checks.check(GenericTicketParser.parse(page, false, "AVAILABLE", 100).offers.isEmpty(), "Generic parser rejects evidence for a different event");
        checks.check(GenericTicketParser.parse(page, true, "BLOCKED", 100).categories.isEmpty(), "Blocked pages cannot contribute category evidence");
        checks.check(GenericTicketParser.parse(page, true, "UNAVAILABLE", 100).offers.isEmpty(), "Conflicting unavailable page cannot supply resale offers");
        checks.check(!GenericTicketParser.parse(new JSONObject().put("valid", false), true, "UNKNOWN", 100).verified,
                "Unsupported layout does not become verified evidence");
        JSONObject ambiguous = payload();
        ambiguous.getJSONArray("rows").getJSONObject(0).put("price", "$625.50");
        TicketData.Snapshot uncertain = GenericTicketParser.parse(ambiguous, true, "UNKNOWN", 100);
        checks.check(uncertain.offers.isEmpty() || (uncertain.offers.get(0).price.isEmpty() && uncertain.offers.get(0).currency.isEmpty()),
                "Ambiguous dollar symbol cannot become an invented currency quote");
        JSONObject conflict = payload();
        conflict.getJSONArray("rows").getJSONObject(0).put("price", "$625.50 EUR");
        TicketData.Snapshot conflicting = GenericTicketParser.parse(conflict, true, "UNKNOWN", 100);
        checks.check(conflicting.offers.get(0).price.isEmpty() && conflicting.offers.get(0).currency.isEmpty(),
                "Dollar symbol with EUR cannot become a currency quote");
        conflict.getJSONArray("rows").getJSONObject(0).put("price", "625,50 kr PLN");
        checks.check(GenericTicketParser.parse(conflict, true, "UNKNOWN", 100).offers.get(0).currency.isEmpty(),
                "Ambiguous kr symbol with incompatible PLN remains unknown");
        JSONObject ambiguousQuantity = payload();
        ambiguousQuantity.getJSONArray("rows").getJSONObject(0).put("quantity", "Dostępne 2–4");
        TicketData.Snapshot uncertainCount = GenericTicketParser.parse(ambiguousQuantity, true, "UNKNOWN", 100);
        checks.check(uncertainCount.offers.isEmpty() || uncertainCount.offers.get(0).quantity == null,
                "Availability range cannot become an exact listing quantity");
        TicketData.Snapshot restored = TicketData.snapshotFromJson(TicketData.snapshotToJson(sample));
        checks.check(restored.categories.size() == 1 && restored.offers.size() == 1
                && "ticketmaster-regional".equals(restored.adapter), "Generic source, categories and offers survive serialization");

        WatchStore store = new WatchStore(context);
        Models.Event event = new Models.Event();
        event.name = "Synthetic unrelated sports fixture";
        event.url = "https://www.ticketmaster.pl/event/offline-sports/88224466";
        Models.Watch watch = Models.Watch.fromEvent(event, "Synthetic team");
        store.remove(watch.id);
        try {
            store.save(watch);
            store.observe(watch.id, "AVAILABLE", "Offline built-in adapter", sample);
            Models.Watch saved = new WatchStore(context).get(watch.id);
            checks.check(saved.ticketSnapshot.offers.size() == 1 && saved.ticketSnapshot.categories.size() == 1,
                    "Generic adapter evidence persists without per-event CSS settings");
            checks.check(TicketOfferText.hasCurrentOffers(saved), "Current built-in offers are eligible for ticket display");
            JSONObject categoriesOnly = payload().put("rows", new JSONArray());
            store.observe(watch.id, "UNKNOWN", "Visible category only",
                    GenericTicketParser.parse(categoriesOnly, true, "UNKNOWN", 200));
            saved = store.get(watch.id);
            checks.check("UNKNOWN".equals(saved.state) && saved.ticketSnapshot.offers.isEmpty()
                    && saved.ticketSnapshot.categories.size() == 1, "Primary categories do not establish inventory availability");
            store.observe(watch.id, "BLOCKED", "Offline blocked response");
            checks.check(store.get(watch.id).ticketSnapshot.categories.isEmpty(), "Blocked check clears stale displayed categories");
        } finally { store.remove(watch.id); }
        return checks.assertions;
    }

    private void eventIdentities() {
        String german = "https://www.ticketmaster.de/event/12345";
        String[] same = {
                "https://www.ticketmaster.de/event/show-%7C-package/12345",
                "https://www.ticketmaster.de/event/bj%C3%B6rk/12345",
                "https://ticketmaster.de/event/renamed/12345?language=en",
                "https://www.ticketmaster.de/en/event/new-title/12345",
                "https://www.ticketmaster.de/en-de/new-title/event/12345"
        };
        for (String value : same) check(PageSignals.sameEvent(german, value),
                "Display slugs and supported locale routes preserve the literal event ID: " + value);
        String italian = "https://www.ticketmaster.it/event/qfd2zy7m15y4";
        check(PageSignals.sameEvent(italian, "https://www.ticketmaster.it/biglietti/synthetic-artist-roma/event/qfd2zy7m15y4"),
                "Italian biglietti route matches its alphanumeric event ID");
        check(PageSignals.sameEvent(italian, "https://www.ticketmaster.it/it/biglietti/synthetic-artist-roma/event/QFD2ZY7M15Y4"),
                "Italian locale route and event ID case preserve identity");
        String[] badPaths = {
                "/event/show/%31%32%33%34%35",
                "/event/show%2fother/12345",
                "/event/show%5cother/12345",
                "/event/show%252fother/12345",
                "/event/%2e%2e/12345",
                "/login/../event/12345",
                "/%65vent/show/12345",
                "/event/checkout/12345",
                "/event/%63heckout/12345",
                "/en/event/checkout/12345",
                "/event/show%3fcheckout/12345",
                "/event/show%00/12345",
                "/event/show%80/12345",
                "/english/event/title/12345",
                "/biglietti/title/event/12345",
                "/event/12345?token=not-a-secret"
        };
        for (String path : badPaths) check(!PageSignals.sameEvent(german, "https://www.ticketmaster.de" + path),
                "Encoded structure, reserved routes and token queries cannot prove event identity: " + path);
    }

    private static String primary(String title, String price) {
        return "<li><div data-testid='ticketTypeInfo'><div><svg aria-hidden='true'></svg><div>"
                + "<span>" + title + "<button><span>Details</span></button></span><span>" + price + "</span>"
                + "</div></div></div><div data-testid='quantityStepper'><button disabled>Minus</button>"
                + "<div role='spinbutton' aria-valuenow='0' aria-valuemin='0' aria-valuemax='6'>0</div>"
                + "<button onclick='window.forbiddenClicks++'>Plus</button></div></li>";
    }

    private static String resale(String attributes, String type, String quantity, String price) {
        return "<li role='button' " + attributes + " onclick='window.forbiddenClicks++'><div>"
                + "<dl><div><dt>POZIOM</dt><dd>Trybuna</dd></div><div><dt>Sektor</dt><dd>B12</dd></div>"
                + "<div><dt>Rząd</dt><dd>15</dd></div></dl>"
                + "<div data-testid='ticketTypeInfo'><div><svg aria-hidden='true'></svg><div>"
                + "<span>Bilety Resale</span><span>" + quantity + "</span><span>" + type + "</span><span>" + price + "</span>"
                + "</div></div></div></div></li>";
    }

    private static String document(String eventName, boolean blocked) {
        return "<!doctype html><html><head><meta name='viewport' content='width=device-width,initial-scale=1'></head><body>"
                + "<h1>" + eventName + "</h1>" + (blocked ? "<p>Please complete the captcha to continue</p>" : "")
                + "<script>window.safeClicks=0;window.forbiddenClicks=0;</script>"
                + "<button role='tab'>Zobacz miejsca na mapie</button>"
                + "<button role='tab' onclick=\"window.safeClicks++;document.getElementById('offers').style.display='block';this.setAttribute('aria-selected','true')\">Wybierz najlepsze dostępne miejsca</button>"
                + "<div id='offers' style='display:none'><ul>"
                + primary("Reserved seating", "190,00 zł–450,50 zł")
                + primary("Standing", "325,00 zł")
                + resale("", "Official Platinum Ticket", "Dostępne 2", "625,50 zł")
                + resale("style='display:none'", "Hidden ticket", "Dostępne 8", "1,00 zł")
                + resale("aria-disabled='true'", "Disabled ticket", "Dostępne 4", "1,00 zł")
                + "</ul><button data-testid='findTicketsBtn' onclick='window.forbiddenClicks++'>Wyszukaj bilety</button></div></body></html>";
    }

    private static JSONObject evaluate(Instrumentation instrumentation, WebView web, String expression) throws Exception {
        CountDownLatch completed = new CountDownLatch(1);
        String[] answer = new String[1];
        instrumentation.runOnMainSync(() -> web.evaluateJavascript(expression, value -> {
            answer[0] = value;
            completed.countDown();
        }));
        if (!completed.await(20, TimeUnit.SECONDS)) throw new AssertionError("Offline generic extraction timed out");
        Object decoded = new JSONTokener(answer[0]).nextValue();
        return decoded instanceof String ? new JSONObject((String) decoded) : (JSONObject) decoded;
    }

    private static void load(Instrumentation instrumentation, WebView web, String url, String html) throws Exception {
        CountDownLatch loaded = new CountDownLatch(1);
        instrumentation.runOnMainSync(() -> {
            web.setWebViewClient(new WebViewClient() {
                @Override public void onPageFinished(WebView view, String location) { loaded.countDown(); }
            });
            web.loadDataWithBaseURL(url, html, "text/html", "UTF-8", null);
        });
        if (!loaded.await(30, TimeUnit.SECONDS)) throw new AssertionError("Offline generic fixture failed to load");
        instrumentation.waitForIdleSync();
    }

    public static int browser(Instrumentation instrumentation) throws Exception {
        GenericTicketChecks checks = new GenericTicketChecks();
        Context context = instrumentation.getTargetContext();
        Activity activity = instrumentation.startActivitySync(new Intent(context, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        WebView[] web = new WebView[1];
        try {
            instrumentation.runOnMainSync(() -> {
                web[0] = new WebView(activity);
                web[0].getSettings().setJavaScriptEnabled(true);
                web[0].getSettings().setBlockNetworkLoads(true);
                activity.setContentView(web[0], new LinearLayout.LayoutParams(-1, -1));
            });
            String[] names = {"Aurora Ensemble — synthetic fixture", "City Derby — synthetic fixture"};
            String[] urls = {"https://www.ticketmaster.pl/event/ensemble/11002233", "https://www.ticketmaster.pl/event/derby/99887766"};
            for (int i = 0; i < names.length; i++) {
                load(instrumentation, web[0], urls[i], document(names[i], false));
                JSONObject action = evaluate(instrumentation, web[0], GenericTicketParser.prepareScript());
                checks.check(action.optBoolean("clicked"), "Generic flow opens the best-available tab for arbitrary events");
                JSONObject extraction = evaluate(instrumentation, web[0], GenericTicketParser.extractionScript());
                TicketData.Snapshot snapshot = GenericTicketParser.parse(extraction, true, "UNKNOWN", 100 + i);
                checks.check(snapshot.categories.size() == 2 && snapshot.offers.size() == 1,
                        "Regional DOM yields categories and only visible enabled resale rows");
                checks.check("625.5".equals(snapshot.offers.get(0).price)
                        && Integer.valueOf(2).equals(snapshot.offers.get(0).quantity), "Regional DOM extracts localized price and listing quantity");
                checks.check("190".equals(snapshot.categories.get(0).min) && "450.5".equals(snapshot.categories.get(0).max),
                        "Regional DOM retains the primary price range");
                checks.check(Integer.valueOf(6).equals(snapshot.categories.get(0).selectableQuantity),
                        "Regional DOM labels spinbutton maximum as selectable capacity");
                JSONObject counters = evaluate(instrumentation, web[0], "({safe:window.safeClicks,forbidden:window.forbiddenClicks})");
                checks.check(counters.optInt("safe") == 1 && counters.optInt("forbidden") == 0,
                        "Generic preparation does not click quantity, reservation or listing controls");
                checks.check(GenericTicketParser.parse(extraction, false, "UNKNOWN", 100).offers.isEmpty(),
                        "Identity guard still applies after generic page extraction");
            }
            load(instrumentation, web[0], urls[0], document(names[0], true));
            JSONObject action = evaluate(instrumentation, web[0], GenericTicketParser.prepareScript());
            checks.check(action.optBoolean("blocked") && !action.optBoolean("clicked"), "Challenge text prevents automatic page preparation");
            JSONObject counters = evaluate(instrumentation, web[0], "({safe:window.safeClicks,forbidden:window.forbiddenClicks})");
            checks.check(counters.optInt("safe") == 0 && counters.optInt("forbidden") == 0, "Blocked page remains untouched");
            load(instrumentation, web[0], urls[1], "<!doctype html><html><body><h1>Different event layout</h1><p>Tickets from 50 PLN</p></body></html>");
            JSONObject unknown = evaluate(instrumentation, web[0], GenericTicketParser.extractionScript());
            TicketData.Snapshot unsupported = GenericTicketParser.parse(unknown, true, "UNKNOWN", 100);
            checks.check(!unsupported.verified && unsupported.offers.isEmpty() && unsupported.categories.isEmpty(),
                    "Unsupported event layout remains unknown instead of scraping arbitrary prices");
            Models.Watch preview = new Models.Watch();
            preview.name = "Aurora Ensemble — synthetic preview";
            preview.state = "AVAILABLE";
            preview.ticketSnapshot = GenericTicketParser.parse(payload(), true, "UNKNOWN", System.currentTimeMillis());
            preview.checkedAt = preview.ticketSnapshot.observedAt;
            java.io.File directory = new java.io.File(activity.getExternalFilesDir(null), "ui-previews");
            directory.mkdirs();
            instrumentation.runOnMainSync(() -> {
                LinearLayout view = new LinearLayout(activity);
                view.setOrientation(LinearLayout.VERTICAL);
                view.setPadding(20, 24, 20, 24);
                view.setBackgroundColor(android.graphics.Color.rgb(9,17,29));
                android.widget.TextView title = new android.widget.TextView(activity);
                title.setText("SYNTHETIC PREVIEW\nAurora Ensemble");
                title.setTextSize(22); title.setTextColor(android.graphics.Color.WHITE);
                view.addView(title);
                TicketViews.addSnapshot(activity, view, preview, false);
                activity.setContentView(view);
            });
            instrumentation.waitForIdleSync();
            instrumentation.runOnMainSync(() -> {
                android.view.View view = activity.getWindow().getDecorView();
                android.graphics.Bitmap bitmap = android.graphics.Bitmap.createBitmap(view.getWidth(), view.getHeight(), android.graphics.Bitmap.Config.ARGB_8888);
                view.draw(new android.graphics.Canvas(bitmap));
                try (java.io.FileOutputStream out = new java.io.FileOutputStream(new java.io.File(directory,"android-generic-tickets.png"))) {
                    bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, out);
                } catch (Exception e) { throw new IllegalStateException(e); }
                bitmap.recycle();
            });
        } finally {
            instrumentation.runOnMainSync(() -> { if (web[0] != null) web[0].destroy(); activity.finish(); });
            instrumentation.waitForIdleSync();
        }
        return checks.assertions;
    }
}
