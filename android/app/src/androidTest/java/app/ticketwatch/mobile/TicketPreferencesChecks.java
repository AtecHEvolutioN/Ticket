package app.ticketwatch.mobile;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.net.Uri;
import android.view.View;
import android.view.ViewGroup;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.ScrollView;
import android.widget.TextView;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

/** Preference checks use synthetic events and never open an event page or send alerts. */
public final class TicketPreferencesChecks {
    private static final String EVENT = "https://www.ticketmaster.cz/event/synthetic-preferences/88770901";
    private static final String OTHER = "https://www.ticketmaster.cz/event/synthetic-preferences/88770902";
    private int assertions;
    private void check(boolean condition, String label) {
        assertions++;
        if (!condition) throw new AssertionError(label);
    }
    private void rejects(Runnable action, String label) {
        boolean rejected = false;
        try { action.run(); } catch (IllegalArgumentException | IllegalStateException expected) { rejected = true; }
        check(rejected, label);
    }
    private static void restore(SharedPreferences preferences, String prior) {
        SharedPreferences.Editor edit = preferences.edit();
        if (prior == null) edit.remove("document"); else edit.putString("document", prior);
        if (!edit.commit()) throw new AssertionError("Restore preference test state");
    }

    public static int run(Context context) throws Exception {
        TicketPreferencesChecks checks = new TicketPreferencesChecks();
        SharedPreferences raw = context.getSharedPreferences("ticket_preferences", Context.MODE_PRIVATE);
        String prior = raw.getString("document", null);
        WatchStore watches = new WatchStore(context);
        Models.Event event = new Models.Event();
        event.name = "Synthetic preference isolation fixture";
        event.url = EVENT;
        Models.Watch watch = Models.Watch.fromEvent(event, "Synthetic preference artist");
        watches.remove(watch.id);
        try {
            raw.edit().remove("document").commit();
            TicketPreferencesStore store = new TicketPreferencesStore(context);
            TicketPreferences defaults = store.defaults();
            checks.check(defaults.quantity == 2 && defaults.requireAdjacent && "cheapest".equals(defaults.priceStrategy),
                    "New preferences default to two adjacent cheapest tickets");
            checks.check(defaults.maxTotalMinor == null && "CZK".equals(defaults.currency)
                            && defaults.allowedCategoryIds.isEmpty() && defaults.allowedTicketTypes.isEmpty()
                            && defaults.allowedSections.isEmpty() && defaults.allowedRows.isEmpty(),
                    "Default budget is optional and all exact-label filters start empty");
            checks.check(!raw.contains("document"), "Reading new defaults does not rewrite existing application state");
            checks.check(TicketPreferences.parseBudget("1234,50", "CZK") == 123450L,
                    "Czech comma prices are converted to integer minor units");
            checks.check(TicketPreferences.parseBudget("1.234", "KWD") == 1234L
                            && TicketPreferences.parseBudget("125", "JPY") == 125L,
                    "Currency fraction digits govern exact budget conversion");
            checks.check(TicketPreferences.parseBudget(" ", "CZK") == null, "Blank budget remains unset");
            for (String invalid : Arrays.asList("-1", "0", "1e3", "1.001", "NaN", "1,000.00", "90071992547409.92"))
                checks.rejects(() -> TicketPreferences.parseBudget(invalid, "CZK"), "Reject invalid total budget " + invalid);
            checks.rejects(() -> TicketPreferences.parseBudget("1.5", "JPY"), "Zero-fraction currency rejects fractional budget");
            checks.rejects(() -> TicketPreferences.parseBudget("20", "ZZZ"), "Unknown currency is rejected");
            for (Object quantity : Arrays.asList(0, 21, 1.5, "2")) {
                JSONObject value = defaults.toJson().put("quantity", quantity);
                checks.rejects(() -> TicketPreferences.fromJson(value), "Reject invalid ticket quantity");
            }
            JSONObject unsupported = defaults.toJson().put("purchase_now", true);
            checks.rejects(() -> TicketPreferences.fromJson(unsupported), "Unknown fields never authorize hidden actions");
            JSONObject strategy = defaults.toJson().put("price_strategy", "any_price");
            checks.rejects(() -> TicketPreferences.fromJson(strategy), "Unsupported price strategy is rejected");
            checks.check(TicketPreferences.lines("Standard\nstandard\nVIP").size() == 2,
                    "Human filter labels deduplicate case-insensitively");
            checks.check(TicketPreferences.lines("Standard\u00a0Seat\nstandard seat").size() == 1,
                    "Unicode whitespace labels deduplicate consistently with the desktop planner");
            JSONObject blankLabel = defaults.toJson().put("allowed_sections", new JSONArray().put("\u00a0"));
            checks.rejects(() -> TicketPreferences.fromJson(blankLabel), "Whitespace-only Unicode labels are rejected");
            for (String control : Arrays.asList("A\u0085B", "A\u009fB")) {
                JSONObject unsafe = defaults.toJson().put("allowed_rows", new JSONArray().put(control));
                checks.rejects(() -> TicketPreferences.fromJson(unsafe), "C1 control characters cannot enter exported filters");
            }
            checks.check(TicketPreferences.lines("A\na", true).size() == 2, "Category identifiers retain exact case");
            defaults.quantity = 4;
            defaults.maxTotalMinor = 600000L;
            defaults.allowedSections = Arrays.asList("110", "112");
            store.saveDefaults(defaults);
            checks.check(new TicketPreferencesStore(context).effective(EVENT).quantity == 4,
                    "Global quantity changes persist and apply to an unconfigured event");
            watches.save(watch);
            watches.observe(watch.id, "UNKNOWN", "Synthetic unchanged observation");
            long checkedAt = watches.get(watch.id).checkedAt;
            TicketPreferences special = new TicketPreferences();
            special.quantity = 3;
            special.maxTotalMinor = 825000L;
            special.allowedCategoryIds = Arrays.asList("2");
            special.allowedTicketTypes = Arrays.asList("Ověřené přeprodávané vstupenky");
            special.allowedSections = Arrays.asList("110");
            special.allowedRows = Arrays.asList("12");
            store.saveOverride(EVENT, special);
            defaults.quantity = 5;
            store.saveDefaults(defaults);
            TicketPreferencesStore reopened = new TicketPreferencesStore(context);
            checks.check(reopened.hasOverride(EVENT) && reopened.effective(EVENT).quantity == 3,
                    "An event keeps its explicit quantity when global defaults change");
            checks.check(reopened.effective(OTHER).quantity == 5, "Sibling events continue to inherit global defaults");
            String equivalent = "https://ticketmaster.cz/event/renamed-preference-event/88770901?language=en";
            checks.check(reopened.hasOverride(equivalent) && reopened.effective(equivalent).quantity == 3,
                    "Region and event identity bind overrides across slug, hostname and language aliases");
            checks.check(!reopened.hasOverride("https://www.ticketmaster.pl/event/synthetic/88770901"),
                    "Same numeric event ID in a different market cannot inherit an override");
            checks.check(watches.get(watch.id).checkedAt == checkedAt && "UNKNOWN".equals(watches.get(watch.id).state),
                    "Editing selection preferences preserves the event's observed inventory");
            for (String invalid : Arrays.asList("https://www.ticketmaster.cz/artist/88770901",
                    "https://secure.ticketmaster.cz/payment/?t=synthetic", "https://on.fgtix.com/trk/demo",
                    "https://secure.ticketmaster.cz/event/12345",
                    "https://www.ticketmaster.cz/event/show%C2%85/12345"))
                checks.rejects(() -> store.saveOverride(invalid, special), "Only supported public event identities accept overrides");
            JSONObject exported = new JSONObject(store.exportJson());
            checks.check(exported.getInt("schema_version") == 1
                            && exported.getJSONObject("defaults").getInt("quantity") == 5
                            && exported.getJSONArray("event_overrides").length() == 1,
                    "Export preserves version, global defaults and explicit event overrides");
            JSONObject effective = exported.getJSONArray("event_overrides").getJSONObject(0).getJSONObject("preferences");
            checks.check(effective.getLong("max_total_minor") == 825000L
                            && effective.getJSONArray("allowed_ticket_types").getString(0).contains("Ověřené")
                            && effective.getJSONArray("allowed_sections").getString(0).equals("110")
                            && effective.getJSONArray("allowed_rows").getString(0).equals("12"),
                    "Export retains exact Unicode type labels and seat filters with an integer total budget");
            checks.check(!store.exportJson().contains("token") && !store.exportJson().contains("api_key")
                            && !store.exportJson().contains("reservation"), "Export carries no credentials or reservation authorization");
            File directory = new File(context.getExternalFilesDir(null), "ui-previews");
            if (!directory.isDirectory() && !directory.mkdirs()) throw new AssertionError("Create preference fixture export directory");
            try (FileOutputStream output = new FileOutputStream(new File(directory, "android-ticket-preferences.json"))) {
                output.write(store.exportJson().getBytes(StandardCharsets.UTF_8));
            }
            store.clearOverride(equivalent);
            checks.check(!store.hasOverride(EVENT) && store.effective(EVENT).quantity == 5,
                    "Resetting an event removes its override and restores current global defaults");
            String valid = raw.getString("document", "");
            raw.edit().putString("document", "invalid synthetic document").commit();
            checks.rejects(store::defaults, "Corrupt saved preferences are reported, never silently reset");
            checks.check("invalid synthetic document".equals(raw.getString("document", "")),
                    "Failed preference reads preserve the original saved data for recovery");
            raw.edit().putString("document", valid).commit();
            JSONObject duplicate = new JSONObject(valid);
            duplicate.put("event_overrides", new JSONArray()
                    .put(new JSONObject().put("event_url", EVENT).put("preferences", special.toJson()))
                    .put(new JSONObject().put("event_url", "https://ticketmaster.cz/event/renamed/88770901").put("preferences", special.toJson())));
            raw.edit().putString("document", duplicate.toString()).commit();
            checks.rejects(store::defaults, "Duplicate saved event identities fail closed");
            TicketPreferences large = new TicketPreferences();
            for (int index = 0; index < 100; index++) {
                String prefix = "x" + index;
                char[] padding = new char[127 - prefix.length()];
                Arrays.fill(padding, 'x');
                String label = prefix + new String(padding);
                large.allowedCategoryIds.add(label);
                large.allowedTicketTypes.add(label);
                large.allowedSections.add(label);
                large.allowedRows.add(label);
            }
            JSONArray many = new JSONArray();
            for (int index = 0; index < 4; index++) many.put(new JSONObject()
                    .put("event_url", "https://www.ticketmaster.cz/event/synthetic-capacity/" + (88770910 + index))
                    .put("preferences", large.toJson()));
            JSONObject nearLimit = new JSONObject().put("schema_version", 1)
                    .put("defaults", large.toJson()).put("event_overrides", many);
            checks.check(nearLimit.toString().getBytes(StandardCharsets.UTF_8).length <= 256 * 1024
                            && nearLimit.toString(2).getBytes(StandardCharsets.UTF_8).length > 256 * 1024,
                    "Capacity fixture distinguishes valid stored JSON from oversized pretty formatting");
            raw.edit().putString("document", nearLimit.toString()).commit();
            String boundedExport = store.exportJson();
            checks.check(boundedExport.getBytes(StandardCharsets.UTF_8).length <= 256 * 1024,
                    "Export remains within the desktop file limit even near storage capacity");
            JSONObject allEntries = new JSONObject(boundedExport);
            checks.check(allEntries.getJSONArray("event_overrides").length() == 4
                            && allEntries.getJSONObject("defaults").getJSONArray("allowed_rows").length() == 100,
                    "Compact export fallback preserves every configured event and filter");
        } finally {
            watches.remove(watch.id);
            restore(raw, prior);
        }
        return checks.assertions;
    }

    public static int ui(Instrumentation instrumentation) throws Exception {
        TicketPreferencesChecks checks = new TicketPreferencesChecks();
        Context context = instrumentation.getTargetContext();
        SharedPreferences raw = context.getSharedPreferences("ticket_preferences", Context.MODE_PRIVATE);
        String prior = raw.getString("document", null);
        TicketPreferencesStore store = new TicketPreferencesStore(context);
        WatchStore watches = new WatchStore(context);
        Models.Event event = new Models.Event();
        event.name = "Synthetic preference UI event";
        event.url = EVENT;
        event.venue = "Synthetic arena";
        event.city = "Prague";
        event.country = "CZ";
        Models.Watch watch = Models.Watch.fromEvent(event, "Synthetic artist");
        watches.remove(watch.id);
        Activity global = null, local = null;
        Instrumentation.ActivityMonitor exportMonitor = null;
        try {
            raw.edit().remove("document").commit();
            watches.save(watch);
            global = instrumentation.startActivitySync(new Intent(context, TicketPreferencesActivity.class)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            instrumentation.waitForIdleSync();
            final Activity defaultsActivity = global;
            onUi(instrumentation, () -> {
                checks.check("2".equals(input(defaultsActivity, "pref_quantity").getText().toString()),
                        "Real preferences UI initially shows two tickets");
                checks.check(((CheckBox) tagged(defaultsActivity, "pref_adjacent")).isChecked(),
                        "Real preferences UI initially requires adjacent seats");
                for (String invalid : Arrays.asList("-2", "100")) {
                    input(defaultsActivity, "pref_quantity").setText(invalid);
                    checks.check(invalid.equals(input(defaultsActivity, "pref_quantity").getText().toString()),
                            "Quantity input preserves invalid pasted values instead of changing the requested count");
                    tagged(defaultsActivity, "pref_save").performClick();
                    checks.check(store.defaults().quantity == 2,
                            "Negative and out-of-range UI quantities cannot overwrite the default count");
                }
                input(defaultsActivity, "pref_quantity").setText("4");
                input(defaultsActivity, "pref_max_total").setText("1.001");
                tagged(defaultsActivity, "pref_save").performClick();
                checks.check(store.defaults().quantity == 2 && store.defaults().maxTotalMinor == null,
                        "Invalid UI price input prevents the whole preference edit from saving");
                checks.check(((TextView) tagged(defaultsActivity, "pref_status")).getText().toString().contains("decimal"),
                        "Invalid price input produces an actionable inline validation message");
                for (String invalid : Arrays.asList("-10", "1e3")) {
                    input(defaultsActivity, "pref_max_total").setText(invalid);
                    checks.check(invalid.equals(input(defaultsActivity, "pref_max_total").getText().toString()),
                            "Budget UI preserves invalid pasted input for validation without changing its meaning");
                    tagged(defaultsActivity, "pref_save").performClick();
                    checks.check(store.defaults().quantity == 2 && store.defaults().maxTotalMinor == null,
                            "Negative or malformed UI totals are rejected without updating preferences");
                }
                input(defaultsActivity, "pref_max_total").setText("1234,50");
                checks.check("1234,50".equals(input(defaultsActivity, "pref_max_total").getText().toString()),
                        "Budget UI preserves a comma decimal separator rather than silently changing the amount");
                tagged(defaultsActivity, "pref_save").performClick();
                checks.check(store.defaults().maxTotalMinor == 123450L,
                        "Comma-decimal budget input saves the intended exact total");
                input(defaultsActivity, "pref_max_total").setText("6000.50");
                input(defaultsActivity, "pref_ticket_types").setText("Standard");
                input(defaultsActivity, "pref_sections").setText("110\n112");
                input(defaultsActivity, "pref_rows").setText("12");
                input(defaultsActivity, "pref_category_ids").setText("2");
                tagged(defaultsActivity, "pref_save").performClick();
                checks.check(store.defaults().quantity == 4 && store.defaults().maxTotalMinor == 600050L,
                        "Real UI saves chosen quantity and an exact total including fees");
            });
            capture(instrumentation, global, "android-ticket-preferences.png");
            onUi(instrumentation, () -> scrollView(defaultsActivity.getWindow().getDecorView()).fullScroll(View.FOCUS_DOWN));
            capture(instrumentation, global, "android-ticket-filters.png");
            onUi(instrumentation, global::finish);
            global = null;
            local = instrumentation.startActivitySync(new Intent(context, TicketPreferencesActivity.class)
                    .putExtra("watch_id", watch.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            instrumentation.waitForIdleSync();
            final Activity eventActivity = local;
            onUi(instrumentation, () -> {
                checks.check(((CheckBox) tagged(eventActivity, "pref_use_defaults")).isChecked()
                                && !input(eventActivity, "pref_quantity").isEnabled(),
                        "A saved event initially inherits defaults and disables override editing");
                checks.check("4".equals(input(eventActivity, "pref_quantity").getText().toString()),
                        "Event UI displays the current global quantity");
                ((CheckBox) tagged(eventActivity, "pref_use_defaults")).setChecked(false);
                checks.check(input(eventActivity, "pref_quantity").isEnabled(), "Turning off inheritance enables event-specific choices");
                input(eventActivity, "pref_quantity").setText("3");
                input(eventActivity, "pref_max_total").setText("8250");
                input(eventActivity, "pref_ticket_types").setText("Ověřené přeprodávané vstupenky");
                input(eventActivity, "pref_sections").setText("110");
                ((CheckBox) tagged(eventActivity, "pref_adjacent")).setChecked(false);
                tagged(eventActivity, "pref_save").performClick();
                checks.check(store.hasOverride(EVENT) && store.effective(EVENT).quantity == 3
                                && !store.effective(EVENT).requireAdjacent && store.defaults().quantity == 4,
                        "Event-specific quantity and adjacency save without changing global defaults");
            });
            capture(instrumentation, local, "android-event-preferences.png");
            File exportedFile = new File(new File(context.getExternalFilesDir(null), "ui-previews"), "android-ticket-preferences-ui.json");
            IntentFilter filter = new IntentFilter(Intent.ACTION_CREATE_DOCUMENT);
            filter.addCategory(Intent.CATEGORY_OPENABLE);
            filter.addDataType("application/json");
            exportMonitor = instrumentation.addMonitor(filter, new Instrumentation.ActivityResult(Activity.RESULT_OK,
                    new Intent().setData(Uri.fromFile(exportedFile))), true);
            onUi(instrumentation, () -> tagged(eventActivity, "pref_export").performClick());
            instrumentation.waitForIdleSync();
            checks.check(exportMonitor.getHits() == 1, "Export button opens the JSON document-save flow");
            checks.check(exportedFile.isFile(), "Successful document selection writes an actual Android preference export");
            byte[] exportBytes = java.nio.file.Files.readAllBytes(exportedFile.toPath());
            JSONObject export = new JSONObject(new String(exportBytes, StandardCharsets.UTF_8));
            checks.check(export.getJSONObject("defaults").getInt("quantity") == 4
                            && export.getJSONArray("event_overrides").getJSONObject(0).getJSONObject("preferences").getInt("quantity") == 3,
                    "UI export contains both saved global defaults and the selected event override");
            instrumentation.removeMonitor(exportMonitor);
            exportMonitor = null;
            onUi(instrumentation, eventActivity::finish);
            local = null;
            local = instrumentation.startActivitySync(new Intent(context, TicketPreferencesActivity.class)
                    .putExtra("watch_id", watch.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            instrumentation.waitForIdleSync();
            final Activity reopened = local;
            onUi(instrumentation, () -> {
                checks.check(!((CheckBox) tagged(reopened, "pref_use_defaults")).isChecked()
                                && "3".equals(input(reopened, "pref_quantity").getText().toString()),
                        "Reopening the event UI restores the explicit saved quantity");
                ((CheckBox) tagged(reopened, "pref_use_defaults")).setChecked(true);
                tagged(reopened, "pref_save").performClick();
                checks.check(!store.hasOverride(EVENT) && store.effective(EVENT).quantity == 4,
                        "Saving Use defaults removes the override and restores global inheritance");
                checks.check(!input(reopened, "pref_quantity").isEnabled(), "Reset event controls show inherited values without editing them");
            });
        } finally {
            if (exportMonitor != null) instrumentation.removeMonitor(exportMonitor);
            if (local != null) { final Activity closing = local; onUi(instrumentation, closing::finish); }
            if (global != null) { final Activity closing = global; onUi(instrumentation, closing::finish); }
            watches.remove(watch.id);
            restore(raw, prior);
            instrumentation.waitForIdleSync();
        }
        return checks.assertions;
    }

    private static View tagged(Activity activity, String name) {
        View view = activity.getWindow().getDecorView().findViewWithTag(name);
        if (view == null) throw new AssertionError("Missing preference control: " + name);
        return view;
    }
    private static EditText input(Activity activity, String name) { return (EditText) tagged(activity, name); }
    private static ScrollView scrollView(View view) {
        if (view instanceof ScrollView) return (ScrollView) view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int index = 0; index < group.getChildCount(); index++) {
                ScrollView found = scrollView(group.getChildAt(index));
                if (found != null) return found;
            }
        }
        return null;
    }
    private static void onUi(Instrumentation instrumentation, Runnable action) {
        Throwable[] failure = new Throwable[1];
        instrumentation.runOnMainSync(() -> { try { action.run(); } catch (Throwable error) { failure[0] = error; } });
        if (failure[0] != null) throw new AssertionError("Preference UI check failed: " + failure[0].getMessage(), failure[0]);
    }
    private static void capture(Instrumentation instrumentation, Activity activity, String name) throws Exception {
        instrumentation.waitForIdleSync();
        Bitmap[] bitmap = new Bitmap[1];
        onUi(instrumentation, () -> {
            View decor = activity.getWindow().getDecorView();
            bitmap[0] = Bitmap.createBitmap(decor.getWidth(), decor.getHeight(), Bitmap.Config.ARGB_8888);
            decor.draw(new Canvas(bitmap[0]));
        });
        File directory = new File(instrumentation.getTargetContext().getExternalFilesDir(null), "ui-previews");
        if (!directory.isDirectory() && !directory.mkdirs()) throw new AssertionError("Create preference UI preview directory");
        try (FileOutputStream output = new FileOutputStream(new File(directory, name))) {
            bitmap[0].compress(Bitmap.CompressFormat.PNG, 100, output);
        } finally { bitmap[0].recycle(); }
    }
}
