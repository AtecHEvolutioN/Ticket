package app.ticketwatch.mobile;

import android.app.Activity;
import android.app.Instrumentation;
import android.app.Notification;
import android.app.NotificationManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.os.Bundle;
import android.service.notification.StatusBarNotification;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.widget.EditText;
import android.widget.TextView;
import java.io.File;
import java.io.FileOutputStream;

/** Runs inside a fresh emulator. No network, genuine credentials, or Discord sends. */
public final class SmokeInstrumentation extends Instrumentation {
    private int assertions;
    private boolean preferencesOnly;
    private void check(boolean condition, String label) {
        assertions++;
        if (!condition) throw new AssertionError(label);
    }

    @Override public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        preferencesOnly = arguments != null && "preferences".equals(arguments.getString("suite"));
        start();
    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        try {
            Context context = getTargetContext();
            if (preferencesOnly) {
                assertions += TicketPreferencesChecks.run(context);
                assertions += TicketPreferencesChecks.ui(this);
                result.putString("stream", "\nPASS: " + assertions + " offline Android preference assertions. No live requests.\n");
                finish(Activity.RESULT_OK, result);
                return;
            }
            urlRules();
            pageSignals();
            catalogSchema();
            watchTransitions(context);
            secureStorage(context);
            assertions += TicketChecks.run(context);
            assertions += TicketChecks.browser(this);
            assertions += GenericTicketChecks.run(context);
            assertions += GenericTicketChecks.browser(this);
            assertions += EuropeTicketChecks.run();
            assertions += EuropeTicketChecks.browser(this);
            assertions += VenueChecks.run(context);
            assertions += VenueChecks.ui(this);
            assertions += TicketPreferencesChecks.run(context);
            assertions += TicketPreferencesChecks.ui(this);
            uiFlow(context);
            result.putString("stream", "\nPASS: " + assertions + " offline Android assertions. No live requests.\n");
            finish(Activity.RESULT_OK, result);
        } catch (Throwable failure) {
            result.putString("stream", "\nFAIL: " + failure.getClass().getSimpleName() + ": " + failure.getMessage() + "\n");
            finish(Activity.RESULT_CANCELED, result);
        }
    }

    private void urlRules() {
        check(!DiscoveryClient.normalizePublicUrl("https://www.ticketmaster.pl/event/korn/123").isEmpty(), "Public Ticketmaster event accepted");
        check(!DiscoveryClient.normalizePublicUrl("https://on.fgtix.com/trk/demo").isEmpty(), "Official Front Gate link accepted for browser");
        check(DiscoveryClient.normalizePublicUrl("https://ticketmaster.pl.evil.invalid/event/1").isEmpty(), "Lookalike domain rejected");
        check(DiscoveryClient.normalizePublicUrl("https://user:secret@www.ticketmaster.pl/event/1").isEmpty(), "Credential URL rejected");
        check(DiscoveryClient.normalizePublicUrl("http://www.ticketmaster.pl/event/1").isEmpty(), "Cleartext link rejected");
        check(DiscoveryClient.normalizePublicUrl("https://secure.ticketmaster.pl/payment/?t=secret").isEmpty(), "Checkout URL rejected");
        check(!DiscoveryClient.normalizePublicUrl("https://www.ticketmaster.pl/event/123?language=en").contains("?"), "Query omitted from stored URL");
    }

    private void watchTransitions(Context context) {
        WatchStore store = new WatchStore(context);
        Models.Event event = new Models.Event();
        event.name = "Offline instrumentation fixture";
        event.url = "https://www.ticketmaster.pl/event/instrumentation/000000";
        event.demo = true;
        Models.Watch watch = Models.Watch.fromEvent(event, "Synthetic artist");
        store.remove(watch.id);
        try {
            store.save(watch);
            check(!store.observe(watch.id, "AVAILABLE", "Initial baseline"), "Initial availability remains quiet");
            check(!store.observe(watch.id, "AVAILABLE", "Repeat"), "Repeated availability deduplicates");
            check(!store.observe(watch.id, "BLOCKED", "Synthetic challenge"), "Blocked does not create an alert");
            check("AVAILABLE".equals(new WatchStore(context).get(watch.id).lastDefinite), "Baseline survives blocked check and store reopen");
            check(!store.observe(watch.id, "AVAILABLE", "Recovery"), "Recovery from blocked does not invent a restock");
            check(!store.observe(watch.id, "UNAVAILABLE", "Negative"), "Negative observation does not alert");
            check(store.observe(watch.id, "AVAILABLE", "Real transition in synthetic data"), "Unavailable-to-available generates one alert");
            check(!store.observe(watch.id, "UNKNOWN", "Incomplete"), "Unknown does not alert");
            check(!new WatchStore(context).observe(watch.id, "AVAILABLE", "Repeat after reopen"), "Deduplication survives store reopen");
            check(!store.observe(watch.id, "UNAVAILABLE", "Negative again"), "New negative baseline");
            check(new WatchStore(context).observe(watch.id, "AVAILABLE", "Restock"), "Later restock generates another alert");
            Models.Watch edit = store.get(watch.id);
            edit.availableSelector = ".new-verified-signal";
            edit.selectorsVerified = true;
            store.save(edit);
            check("UNKNOWN".equals(store.get(watch.id).lastDefinite), "Parser edit resets inventory baseline");
            check(!store.observe(watch.id, "AVAILABLE", "New parser baseline"), "New parser initial result remains quiet");
        } finally { store.remove(watch.id); }
        check(store.get(watch.id) == null, "Synthetic watch removed");
        Models.Watch demo = new Models.Watch();
        demo.id = "instrumentation-empty-demo";
        demo.demo = true;
        demo.name = "Synthetic only";
        demo.url = "";
        try {
            store.save(demo);
            check(new WatchStore(context).get(demo.id).demo, "URL-free demo survives persistence");
        } finally { store.remove(demo.id); }
    }

    private void pageSignals() {
        String expected = "https://www.ticketmaster.pl/event/korn/496543017";
        check(PageSignals.sameEvent(expected, "https://ticketmaster.pl/event/renamed/496543017"), "Slug redirect preserves event identity");
        check(!PageSignals.sameEvent(expected, "https://www.ticketmaster.pl/event/another/999999999"), "Different event cannot supply availability");
        check(!PageSignals.challenge("This site is protected by reCAPTCHA"), "Ordinary reCAPTCHA footer is not a challenge");
        check(PageSignals.challenge("Please complete the captcha to continue"), "Explicit challenge is blocked");
        check("UNKNOWN".equals(PageSignals.classify(true, false, true, true, false, "")), "Unverified selectors cannot prove availability");
        check("AVAILABLE".equals(PageSignals.classify(true, true, true, true, false, "")), "Verified positive signal is available");
        check("UNKNOWN".equals(PageSignals.classify(true, true, true, true, true, "")), "Conflicting signals stay unknown");
        check("UNKNOWN".equals(PageSignals.classify(false, true, true, false, true, "")), "Wrong page cannot prove sold out");
    }

    private void catalogSchema() throws Exception {
        org.json.JSONObject empty = new org.json.JSONObject("{\"page\":{\"number\":0,\"totalElements\":0,\"totalPages\":0}}");
        check(DiscoveryClient.resultItems(empty, "events").length() == 0, "Explicit empty catalog is valid");
        boolean missingRejected = false;
        try {
            DiscoveryClient.resultItems(new org.json.JSONObject("{\"page\":{\"number\":0,\"totalElements\":3,\"totalPages\":1}}"), "events");
        } catch (Exception expected) { missingRejected = true; }
        check(missingRejected, "Missing nonempty catalog is unknown, not zero events");
        boolean faultRejected = false;
        try { DiscoveryClient.resultItems(new org.json.JSONObject("{\"fault\":{}}"), "events"); }
        catch (Exception expected) { faultRejected = true; }
        check(faultRejected, "API fault cannot become empty search results");
    }

    private void secureStorage(Context context) throws Exception {
        SecureStore store = new SecureStore(context);
        String previous = store.get("api_key");
        String testValue = "offline-test-value-not-a-real-key";
        try {
            store.put("api_key", testValue);
            check(testValue.equals(new SecureStore(context).get("api_key")), "Keystore round trip survives new store instance");
            String stored = context.getSharedPreferences("encrypted_credentials", Context.MODE_PRIVATE).getString("api_key", "");
            check(!stored.contains(testValue) && !stored.isEmpty(), "Private preferences contain ciphertext rather than credential text");
            store.remove("api_key");
            check(store.get("api_key").isEmpty(), "Credential removal works");
        } finally {
            if (previous.isEmpty()) store.remove("api_key"); else store.put("api_key", previous);
        }
    }

    private void uiFlow(Context context) throws Exception {
        SharedPreferences prefs = context.getSharedPreferences("ticket_settings", Context.MODE_PRIVATE);
        String oldArtist = prefs.getString("explore_query", "KoRn");
        String oldCountry = prefs.getString("explore_country", "");
        String oldCity = prefs.getString("explore_city", "");
        prefs.edit().putString("explore_query", "KoRn").putString("explore_country", "")
                .putString("explore_city", "").commit();
        Activity activity = startActivitySync(new Intent(context, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        try {
            waitForIdleSync();
            onUi(() -> {
                check(activity.findViewById(R.id.ui_explore).isSelected(), "Explore is selected at launch");
                check(activity.findViewById(R.id.ui_demo).isShown(), "Offline demo is accessible on Explore");
                check(activity.findViewById(R.id.ui_search).isEnabled(), "Search control is usable");
                activity.getWindow().getDecorView().findViewWithTag("PL").performClick();
                EditText country = activity.getWindow().getDecorView().findViewWithTag("explore_country");
                check("PL".contentEquals(country.getText()), "Country chip updates the search filter");
            });
            capture(activity, "android-explore.png");
            click(activity, R.id.ui_settings);
            onUi(() -> {
                check(activity.findViewById(R.id.ui_settings).isSelected(), "Settings navigation changes selection");
                check((activity.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_SECURE) != 0,
                        "Credential screen protects system screenshots");
                EditText input = activity.getWindow().getDecorView().findViewWithTag("settings_api_key");
                check(input.getText().length() == 0, "Settings never displays an existing credential");
            });
            // Draw only this app's blank input views; do not disable FLAG_SECURE for previews.
            capture(activity, "android-settings.png");
            click(activity, R.id.ui_explore);
            onUi(() -> {
                EditText country = activity.getWindow().getDecorView().findViewWithTag("explore_country");
                check("PL".contentEquals(country.getText()), "Search draft survives tab navigation");
            });
            click(activity, R.id.ui_demo);
            onUi(() -> check(activity.findViewById(R.id.ui_watchlist).isSelected(), "Demo opens the Watchlist"));
            click(activity, R.id.ui_demo_unavailable);
            check("UNAVAILABLE".equals(new WatchStore(context).get("demo-korn-local").state), "Demo button saves negative observation");
            click(activity, R.id.ui_demo_available);
            check("AVAILABLE".equals(new WatchStore(context).get("demo-korn-local").state), "Demo button saves positive observation");
            check(new WatchStore(context).get("demo-korn-local").ticketSnapshot.offers.size() == 2, "Demo saves two explicitly synthetic ticket offers");
            onUi(() -> {
                check(containsText(activity.getWindow().getDecorView(), "435.85 PLN"), "Watchlist shows observed price and currency");
                check(containsText(activity.getWindow().getDecorView(), "2 tickets in this listing"), "Watchlist identifies listing quantity");
                check(activity.findViewById(R.id.ui_ticket_details) != null, "Demo exposes offline ticket details");
            });
            NotificationManager manager = (NotificationManager) context.getSystemService(Context.NOTIFICATION_SERVICE);
            boolean demoNotification = false;
            for (StatusBarNotification posted : manager.getActiveNotifications()) {
                if ("Demo ticket alert".contentEquals(posted.getNotification().extras.getCharSequence(Notification.EXTRA_TITLE, ""))) {
                    demoNotification = true;
                }
            }
            check(demoNotification, "Demo unavailable-to-available posts a labelled local alert");
            capture(activity, "android-watchlist.png");
            clickTag(activity, "watch_filter_available");
            onUi(() -> check(activity.findViewById(R.id.ui_demo_available) == null, "Demo does not appear in real availability filter"));
            clickTag(activity, "watch_filter_unchecked");
            onUi(() -> check(activity.findViewById(R.id.ui_demo_available) == null, "Demo does not appear in needs-check filter"));
            clickTag(activity, "watch_filter_all");
            onUi(() -> check(activity.findViewById(R.id.ui_demo_available) != null, "All filter restores the synthetic watch"));

            Intent pageIntent = new Intent(context, PageActivity.class).putExtra("watch_id", "demo-korn-local")
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            Activity inspector = startActivitySync(pageIntent);
            try {
                waitForIdleSync();
                onUi(() -> {
                    check(!inspector.findViewById(R.id.ui_check_page).isEnabled(), "Demo inspector cannot make a live page check");
                    check(!inspector.findViewById(R.id.ui_open_browser).isEnabled(), "URL-free demo cannot open a browser");
                    check(containsText(inspector.getWindow().getDecorView(), "DEMO · SYNTHETIC EVENT"), "Inspector clearly labels synthetic preview");
                    check(containsText(inspector.getWindow().getDecorView(), "Official Platinum"), "Inspector shows second ticket type");
                    check(containsText(inspector.getWindow().getDecorView(), "910.80 PLN"), "Inspector shows decimal price for the second offer");
                    check(containsText(inspector.getWindow().getDecorView(), "Total remaining: not reported"), "Inspector does not add listing quantities into stock");
                });
                capture(inspector, "android-inspector.png");
                capture(inspector, "android-tickets.png");
            } finally { onUi(inspector::finish); }
        } finally {
            onUi(activity::finish);
            waitForIdleSync();
            prefs.edit().putString("explore_query", oldArtist).putString("explore_country", oldCountry)
                    .putString("explore_city", oldCity).commit();
        }
    }

    private void onUi(Runnable action) {
        Throwable[] failure = new Throwable[1];
        runOnMainSync(() -> {
            try { action.run(); }
            catch (Throwable error) { failure[0] = error; }
        });
        if (failure[0] != null) throw new AssertionError("UI check failed", failure[0]);
    }

    private void click(Activity activity, int id) {
        onUi(() -> {
            View view = activity.findViewById(id);
            if (view == null || !view.isEnabled()) throw new AssertionError("Missing or disabled UI control: " + id);
            view.performClick();
        });
        waitForIdleSync();
    }

    private void clickTag(Activity activity, String tag) {
        onUi(() -> {
            View view = activity.getWindow().getDecorView().findViewWithTag(tag);
            if (view == null) throw new AssertionError("Missing UI filter: " + tag);
            view.performClick();
        });
        waitForIdleSync();
    }

    private boolean containsText(View view, String expected) {
        if (view instanceof TextView && expected.contentEquals(((TextView) view).getText())) return true;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) if (containsText(group.getChildAt(i), expected)) return true;
        }
        return false;
    }

    private void capture(Activity activity, String name) throws Exception {
        waitForIdleSync();
        Bitmap[] captured = new Bitmap[1];
        onUi(() -> {
            View decor = activity.getWindow().getDecorView();
            captured[0] = Bitmap.createBitmap(decor.getWidth(), decor.getHeight(), Bitmap.Config.ARGB_8888);
            decor.draw(new Canvas(captured[0]));
        });
        File directory = new File(getTargetContext().getExternalFilesDir(null), "ui-previews");
        if (!directory.isDirectory() && !directory.mkdirs()) throw new java.io.IOException("Cannot save UI preview");
        try (FileOutputStream output = new FileOutputStream(new File(directory, name))) {
            captured[0].compress(Bitmap.CompressFormat.PNG, 100, output);
        } finally { captured[0].recycle(); }
    }
}
