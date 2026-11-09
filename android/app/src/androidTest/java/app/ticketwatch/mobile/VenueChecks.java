package app.ticketwatch.mobile;

import android.app.Activity;
import android.app.Instrumentation;
import android.accessibilityservice.AccessibilityServiceInfo;
import android.content.Context;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.View;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.accessibility.AccessibilityWindowInfo;
import android.widget.TextView;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.File;
import java.io.FileOutputStream;
import java.util.Arrays;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/** Synthetic venue references are tested independently of event inventory and page credentials. */
public final class VenueChecks {
    private int assertions;
    private void check(boolean condition, String label) {
        assertions++;
        if (!condition) throw new AssertionError(label);
    }

    private static Models.Watch watch(String eventId, String venueId, String site, String city, String country) {
        Models.Event event = new Models.Event();
        event.id = eventId;
        event.name = "Synthetic venue event " + eventId;
        event.url = "https://www." + site + "/event/offline-venue/" + eventId;
        event.venueId = venueId;
        event.venue = "Synthetic Venue Reference Arena";
        event.city = city;
        event.country = country;
        return Models.Watch.fromEvent(event, "Synthetic venue artist");
    }

    private static TicketData.Snapshot inventory() throws Exception {
        JSONObject row = new JSONObject().put("type", "Synthetic source-only resale")
                .put("section", "Source-A").put("row", "4").put("quantity", "Dostępne 2").put("price", "625,50 zł");
        JSONObject payload = new JSONObject().put("valid", true).put("adapter", GenericTicketParser.ADAPTER)
                .put("rows", new JSONArray().put(row)).put("categories", new JSONArray());
        return GenericTicketParser.parse(payload, true, "AVAILABLE", System.currentTimeMillis());
    }

    private void discovery() throws Exception {
        String attractionId = "offlineVenueArtist235";
        JSONObject venue = new JSONObject().put("id", "offlineVenueStable235").put("name", "Synthetic Discovery Venue")
                .put("city", new JSONObject().put("name", "Test City"))
                .put("country", new JSONObject().put("countryCode", "PL"));
        JSONObject event = new JSONObject().put("id", "offlineVenueEvent235")
                .put("name", "Synthetic Discovery venue event")
                .put("url", "https://www.ticketmaster.pl/event/offline-venue/88770001")
                .put("_embedded", new JSONObject().put("venues", new JSONArray().put(venue))
                        .put("attractions", new JSONArray().put(new JSONObject().put("id", attractionId))));
        JSONObject root = new JSONObject().put("_embedded", new JSONObject().put("events", new JSONArray().put(event)))
                .put("page", new JSONObject().put("number", 0).put("totalPages", 1).put("totalElements", 1));
        Models.EventPage page = DiscoveryClient.parseEvents(root, attractionId, "PL", "");
        check(page.events.size() == 1 && "offlineVenueStable235".equals(page.events.get(0).venueId),
                "Discovery preserves the official venue ID from its response");
        Models.Watch saved = Models.Watch.fromEvent(page.events.get(0), "Synthetic Discovery artist");
        check("offlineVenueStable235".equals(saved.venueId) && "Synthetic Discovery Venue".equals(saved.venue),
                "Saving an event preserves the venue ID separately from its display name");
    }

    public static int run(Context context) throws Exception {
        VenueChecks checks = new VenueChecks();
        checks.discovery();
        VenueStore venues = new VenueStore(context);
        WatchStore watches = new WatchStore(context);
        Models.Watch first = watch("88770101", "offlineVenueMain235", "ticketmaster.pl", "Test City", "PL");
        Models.Watch second = watch("88770102", "offlineVenueMain235", "ticketmaster.pl", "Test City", "PL");
        second.venue = "Synthetic Renamed Reference Arena";
        Models.Watch differentId = watch("88770103", "offlineVenueOther235", "ticketmaster.pl", "Test City", "PL");
        Models.Watch otherSite = watch("88770104", "offlineVenueMain235", "ticketmaster.cz", "Test City", "PL");
        Models.Watch otherCity = watch("88770105", "offlineVenueMain235", "ticketmaster.pl", "Different Test City", "PL");
        Models.Watch otherCountry = watch("88770106", "offlineVenueMain235", "ticketmaster.pl", "Test City", "CZ");
        Models.Watch fallback = watch("88770107", "", "ticketmaster.pl", "Test City", "PL");
        Models.Watch composed = watch("88770108", "", "ticketmaster.se", "Örebro Fixture", "se");
        composed.venue = "Synthetic Café Venue";
        Models.Watch decomposed = watch("88770109", "", "ticketmaster.se", "Örebro Fixture", "SE");
        decomposed.venue = "Synthetic Cafe\u0301 Venue";
        Models.Watch caseChanged = watch("88770110", "", "ticketmaster.se", "Örebro Fixture", "SE");
        caseChanged.venue = "Synthetic CAFÉ Venue";
        caseChanged.city = "  öREBRO   fixture  ";
        Models.Watch missingCity = watch("88770111", "", "ticketmaster.pl", "", "PL");
        Models.Watch missingName = watch("88770112", "", "ticketmaster.pl", "Test City", "PL");
        missingName.venue = "";
        Models.Watch missingCountry = watch("88770113", "", "ticketmaster.pl", "Test City", "");
        Models.Watch promotionFallback = watch("88770114", "", "ticketmaster.pl", "Promotion Test City", "PL");
        promotionFallback.venue = "  Synthetic   Promotion Café Arena  ";
        Models.Watch promotionOfficial = watch("88770115", "offlinePromotionIdA235", "ticketmaster.pl", "promotion   test city", "pl");
        promotionOfficial.venue = "synthetic promotion cafe\u0301 arena";
        Models.Watch promotionSecondId = watch("88770116", "offlinePromotionIdB235", "ticketmaster.pl", "Promotion Test City", "PL");
        promotionSecondId.venue = "Synthetic Promotion Café Arena";
        Models.Watch differentSpelling = watch("88770117", "", "ticketmaster.se", "Örebro Fixture", "SE");
        differentSpelling.venue = "Synthetic Cafe Venue";
        Models.Watch collisionFallback = watch("88770118", "", "ticketmaster.pl", "Collision Test City", "PL");
        collisionFallback.venue = "Synthetic Original Collision Hall";
        Models.Watch collisionOfficial = watch("88770119", "offlineCollisionId235", "ticketmaster.pl", "Collision Test City", "PL");
        collisionOfficial.venue = collisionFallback.venue;
        Models.Watch collisionRenamed = watch("88770120", "offlineCollisionId235", "ticketmaster.pl", "Collision Test City", "PL");
        collisionRenamed.venue = "Synthetic Renamed Collision Hall";
        Models.Watch[] fixtures = {first, second, differentId, otherSite, otherCity, otherCountry, fallback,
                composed, decomposed, caseChanged, missingCity, missingName, missingCountry,
                promotionFallback, promotionOfficial, promotionSecondId, differentSpelling,
                collisionFallback, collisionOfficial, collisionRenamed};
        Set<String> venueKeys = new HashSet<>();
        for (Models.Watch fixture : fixtures) {
            String key = VenueStore.keyFor(fixture);
            if (!key.isEmpty()) venueKeys.add(key);
            watches.remove(fixture.id);
        }
        for (String key : venueKeys) venues.removeVenue(key);
        try {
            checks.check(!VenueStore.keyFor(first).isEmpty() && VenueStore.keyFor(first).equals(VenueStore.keyFor(second)),
                    "Different events at the same official venue share its reference identity");
            checks.check(!VenueStore.keyFor(first).equals(VenueStore.keyFor(differentId)),
                    "Different official venue IDs never merge by display name");
            checks.check(!VenueStore.keyFor(first).equals(VenueStore.keyFor(otherSite)),
                    "The Ticketmaster site is part of venue identity");
            VenueStore.Venue registered = venues.register(first);
            checks.check(registered != null && registered.key.equals(VenueStore.keyFor(first))
                            && "offlineVenueMain235".equals(registered.venueId), "Official venue registration retains its source ID");
            checks.check(venues.register(second) != null && venues.forWatch(second) != null,
                    "The same official venue can be reused despite a changed display name");
            checks.check(venues.forWatch(otherCity) == null && venues.register(otherCity) == null,
                    "Conflicting nonempty city metadata blocks official-ID reuse");
            checks.check(venues.forWatch(otherCountry) == null && venues.register(otherCountry) == null,
                    "Conflicting nonempty country metadata blocks official-ID reuse");
            checks.check(venues.forWatch(first) != null && "Test City".equals(venues.forWatch(first).city),
                    "Conflicting registration cannot overwrite the existing venue location");
            checks.check(venues.forWatch(differentId) == null && venues.forWatch(otherSite) == null,
                    "An unregistered different ID or site cannot reuse an existing venue");
            checks.check(VenueStore.keyFor(composed).equals(VenueStore.keyFor(decomposed)),
                    "Fallback names use NFC and country codes use uppercase normalization");
            checks.check(VenueStore.keyFor(composed).equals(VenueStore.keyFor(caseChanged)),
                    "Venue name and city identity normalize whitespace and letter case");
            checks.check(!VenueStore.keyFor(composed).equals(VenueStore.keyFor(differentSpelling)),
                    "Venue identity does not use fuzzy matching or strip meaningful accents");
            checks.check("Synthetic Café Venue".equals(venues.register(composed).name),
                    "Normalized identity does not lowercase the venue's display name");
            checks.check(VenueStore.keyFor(missingCity).isEmpty() && venues.register(missingCity) == null,
                    "Fallback identity requires a city");
            checks.check(VenueStore.keyFor(missingName).isEmpty() && venues.register(missingName) == null,
                    "Fallback identity requires a venue name");
            checks.check(VenueStore.keyFor(missingCountry).isEmpty() && venues.register(missingCountry) == null,
                    "Fallback identity requires a country");

            VenueStore.Preset preset = venues.savePreset(first, "Synthetic balcony references", Arrays.asList("A1", "B2"),
                    "Synthetic orientation notes; event seating may change.");
            checks.check(preset != null && preset.venueKey.equals(registered.key)
                            && preset.sectionReferences.equals(Arrays.asList("A1", "B2")), "Preset stores section references under the venue identity");
            VenueStore reopened = new VenueStore(context);
            checks.check(reopened.forWatch(first) != null && reopened.all().stream().anyMatch(v -> v.key.equals(registered.key)),
                    "Registered venues persist across store instances");
            checks.check(reopened.presetsFor(second).size() == 1 && reopened.presetsFor(second).get(0).id.equals(preset.id)
                            && reopened.presetsFor(second).get(0).notes.contains("Synthetic orientation"),
                    "Named venue presets persist and are reusable across events");
            checks.check(reopened.presetsFor(differentId).isEmpty() && reopened.presetsFor(otherSite).isEmpty()
                            && reopened.presetsFor(otherCity).isEmpty(),
                    "Venue presets do not cross site, official-ID or location-conflict boundaries");
            first.availableSelector = ".source-only-ready";
            second.availableSelector = ".target-only-ready";
            second.unavailableSelector = ".target-only-unavailable";
            watches.save(first);
            watches.save(second);
            watches.observe(first.id, "AVAILABLE", "Synthetic source inventory", inventory());
            Models.Watch prior = watches.get(first.id);
            checks.check(reopened.apply(watches, first.id, preset.id), "Preset can be applied to its source event");
            Models.Watch updated = new WatchStore(context).get(first.id);
            checks.check(updated.venueId.equals(first.venueId) && preset.id.equals(updated.venuePresetId),
                    "Official venue ID and selected preset reference survive watch persistence");
            checks.check(updated.checkedAt == prior.checkedAt && updated.state.equals(prior.state)
                            && TicketData.snapshotToJson(updated.ticketSnapshot).toString().equals(TicketData.snapshotToJson(prior.ticketSnapshot).toString()),
                    "Applying a preset preserves the event's existing inventory observation");
            checks.check(reopened.apply(watches, second.id, preset.id), "Another event at the same venue can select the preset");
            Models.Watch target = watches.get(second.id);
            checks.check(preset.id.equals(target.venuePresetId) && reopened.selected(target) != null,
                    "The target event stores only a selected venue-preset reference");
            checks.check("UNKNOWN".equals(target.state) && target.checkedAt == 0 && target.ticketSnapshot.offers.isEmpty()
                            && target.ticketSnapshot.categories.isEmpty() && target.priceRanges.isEmpty(),
                    "Cross-event venue reuse never transfers offers, prices, types or inventory state");
            checks.check(!target.selectorsVerified && !target.offerProfile.verified
                            && ".target-only-ready".equals(target.availableSelector) && ".target-only-unavailable".equals(target.unavailableSelector),
                    "Applying a venue reference never verifies or copies scraping selectors");
            second.name = "Synthetic event with refreshed catalog metadata";
            watches.save(second);
            checks.check(preset.id.equals(watches.get(second.id).venuePresetId),
                    "A metadata-only resave with an empty incoming reference retains the current valid preset");
            checks.check(reopened.apply(watches, second.id, "") && watches.get(second.id).venuePresetId.isEmpty()
                            && reopened.selected(watches.get(second.id)) == null,
                    "The explicit reference-only clear action still removes a selected preset");
            checks.check(reopened.apply(watches, second.id, preset.id), "An explicitly cleared preset can be selected again");
            watches.save(differentId);
            checks.check(!reopened.apply(watches, differentId.id, preset.id) && watches.get(differentId.id).venuePresetId.isEmpty(),
                    "Applying a foreign venue preset is rejected without altering the target");
            reopened.removePreset(preset.id);
            checks.check(new VenueStore(context).selected(watches.get(second.id)) == null
                            && new VenueStore(context).presetsFor(second).isEmpty(), "Deleting a preset invalidates its saved selection");
            checks.check(!reopened.apply(watches, second.id, preset.id), "A deleted preset cannot be reapplied");

            watches.save(promotionFallback);
            VenueStore.Venue fallbackVenue = reopened.register(promotionFallback);
            checks.check(fallbackVenue != null && fallbackVenue.venueId.isEmpty(),
                    "A complete venue location can be registered before its official ID is known");
            venueKeys.add(fallbackVenue.key);
            VenueStore.Preset fallbackPreset = reopened.savePreset(promotionFallback, "Synthetic early venue reference",
                    Arrays.asList("North", "South"), "Created before the official venue ID was known.");
            checks.check(reopened.apply(watches, promotionFallback.id, fallbackPreset.id),
                    "A fallback venue preset can be selected before promotion");
            VenueStore.Venue promoted = reopened.register(promotionOfficial);
            checks.check(promoted != null && promoted.key.equals(fallbackVenue.key)
                            && promotionOfficial.venueId.equals(promoted.venueId),
                    "The first exact matching official ID promotes the fallback without changing its local key");
            VenueStore promotedStore = new VenueStore(context);
            VenueStore.Venue reopenedVenue = promotedStore.forWatch(promotionOfficial);
            checks.check(reopenedVenue != null && reopenedVenue.key.equals(fallbackVenue.key)
                            && promotedStore.presetsFor(promotionOfficial).size() == 1
                            && fallbackPreset.id.equals(promotedStore.presetsFor(promotionOfficial).get(0).id)
                            && fallbackVenue.key.equals(promotedStore.presetsFor(promotionOfficial).get(0).venueKey),
                    "Promotion and existing preset references survive reopening the venue store");
            checks.check(promotedStore.selected(watches.get(promotionFallback.id)) != null,
                    "An unambiguous no-ID watch keeps its selected preset after venue promotion");
            watches.save(promotionOfficial);
            checks.check(promotedStore.apply(watches, promotionOfficial.id, fallbackPreset.id)
                            && promotedStore.selected(watches.get(promotionOfficial.id)) != null,
                    "A new official-ID event can reuse the preset created for the fallback venue");
            VenueStore.Venue secondOfficialVenue = promotedStore.register(promotionSecondId);
            checks.check(secondOfficialVenue != null && !secondOfficialVenue.key.equals(promoted.key)
                            && promotionSecondId.venueId.equals(secondOfficialVenue.venueId)
                            && promotedStore.presetsFor(promotionSecondId).isEmpty(),
                    "A second official ID at the same normalized location never merges with the first");
            venueKeys.add(secondOfficialVenue.key);
            VenueStore ambiguousStore = new VenueStore(context);
            checks.check(ambiguousStore.forWatch(promotionFallback) == null && ambiguousStore.register(promotionFallback) == null
                            && ambiguousStore.presetsFor(promotionFallback).isEmpty(),
                    "A no-ID lookup is ambiguous once two official IDs share the same normalized location");
            checks.check(ambiguousStore.selected(watches.get(promotionFallback.id)) == null
                            && ambiguousStore.selected(watches.get(promotionOfficial.id)) != null,
                    "Ambiguity invalidates only the no-ID selection while the official-ID reference remains usable");

            VenueStore.Venue originalCollisionVenue = reopened.register(collisionFallback);
            VenueStore.Preset collisionPreset = reopened.savePreset(collisionFallback, "Synthetic retained reference",
                    Arrays.asList("Balcony"), "The original venue reference must survive identity changes.");
            VenueStore.Venue identifiedCollisionVenue = reopened.register(collisionOfficial);
            checks.check(identifiedCollisionVenue != null && identifiedCollisionVenue.key.equals(originalCollisionVenue.key),
                    "Collision fixture promotes its fallback while preserving its local key");
            VenueStore.Venue renamedCollisionVenue = reopened.register(collisionRenamed);
            checks.check(renamedCollisionVenue != null && collisionRenamed.venue.equals(renamedCollisionVenue.name)
                            && originalCollisionVenue.key.equals(renamedCollisionVenue.key),
                    "An official-ID venue can be renamed without changing its stable reference key");
            checks.check(reopened.forWatch(collisionFallback) == null && reopened.register(collisionFallback) == null,
                    "A stale fallback key collision cannot recreate or overwrite a renamed official venue");
            VenueStore collisionStore = new VenueStore(context);
            VenueStore.Venue retained = collisionStore.forWatch(collisionOfficial);
            checks.check(retained != null && collisionOfficial.venueId.equals(retained.venueId)
                            && collisionRenamed.venue.equals(retained.name) && originalCollisionVenue.key.equals(retained.key)
                            && collisionStore.presetsFor(collisionOfficial).size() == 1
                            && collisionPreset.id.equals(collisionStore.presetsFor(collisionOfficial).get(0).id),
                    "A rejected stale-key registration preserves the existing official identity and presets");
        } finally {
            for (Models.Watch fixture : fixtures) watches.remove(fixture.id);
            for (String key : venueKeys) venues.removeVenue(key);
        }
        return checks.assertions;
    }

    public static int ui(Instrumentation instrumentation) throws Exception {
        VenueChecks checks = new VenueChecks();
        Context context = instrumentation.getTargetContext();
        VenueStore venues = new VenueStore(context);
        WatchStore watches = new WatchStore(context);
        Models.Watch watch = watch("88770201", "offlineVenueUi235", "ticketmaster.pl", "Fixture City", "PL");
        String venueKey = VenueStore.keyFor(watch);
        watches.remove(watch.id);
        venues.removeVenue(venueKey);
        Activity activity = null;
        try {
            watches.save(watch);
            // Subscribe before creating windows so the automation connection receives
            // the activity and dialog events instead of attaching after their display.
            AccessibilityServiceInfo accessibility = instrumentation.getUiAutomation().getServiceInfo();
            accessibility.flags |= AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS
                    | AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS;
            instrumentation.getUiAutomation().setServiceInfo(accessibility);
            activity = instrumentation.startActivitySync(new Intent(context, VenueActivity.class)
                    .putExtra("watch_id", watch.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            instrumentation.waitForIdleSync();
            final Activity opened = activity;
            instrumentation.runOnMainSync(() -> {
                View add = opened.findViewById(R.id.ui_venue_add);
                checks.check(add != null && add.isEnabled(), "A saved venue exposes the Add preset action");
                add.performClick();
            });
            instrumentation.waitForIdleSync();
            setDialogText(instrumentation, "ui_venue_preset_name", "Synthetic upper-level references");
            setDialogText(instrumentation, "ui_venue_sections", "201\n202");
            setDialogText(instrumentation, "ui_venue_notes", "Synthetic example only. Confirm the event-specific seating map.");
            performAccessibilityAction(instrumentation, "android:id/button1", AccessibilityNodeInfo.ACTION_CLICK, null);
            instrumentation.waitForIdleSync();
            List<VenueStore.Preset> created = venues.presetsFor(watch);
            checks.check(created.size() == 1 && "Synthetic upper-level references".equals(created.get(0).name)
                            && created.get(0).sectionReferences.equals(Arrays.asList("201", "202"))
                            && created.get(0).notes.contains("Synthetic example only"),
                    "The Add preset form saves its name, separate section lines and notes");
            VenueStore.Preset preset = created.get(0);
            instrumentation.runOnMainSync(() -> {
                View button = opened.getWindow().getDecorView().findViewWithTag("venue_use_" + preset.id);
                checks.check(button != null && button.isEnabled(), "The venue manager exposes a reusable preset for its selected event");
                button.performClick();
            });
            instrumentation.waitForIdleSync();
            checks.check(preset.id.equals(new WatchStore(context).get(watch.id).venuePresetId),
                    "The venue manager persists its selected preset reference");
            instrumentation.runOnMainSync(() -> {
                TextView selected = opened.findViewById(R.id.ui_venue_selected);
                checks.check(selected != null && selected.getText().toString().contains(preset.name),
                        "The venue manager displays the selected preset by name");
                savePreview(opened);
            });
            Models.Watch selected = new WatchStore(context).get(watch.id);
            checks.check("UNKNOWN".equals(selected.state) && selected.ticketSnapshot.offers.isEmpty()
                            && !selected.selectorsVerified && !selected.offerProfile.verified,
                    "The UI preset action does not create inventory or verified page rules");
            venues.removePreset(preset.id);
            checks.check(new VenueStore(context).selected(new WatchStore(context).get(watch.id)) == null,
                    "Removing a preset invalidates the UI-selected venue reference");
            instrumentation.runOnMainSync(opened::finish);
            instrumentation.waitForIdleSync();
            activity = instrumentation.startActivitySync(new Intent(context, VenueActivity.class)
                    .putExtra("watch_id", watch.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            instrumentation.waitForIdleSync();
            final Activity reopened = activity;
            instrumentation.runOnMainSync(() -> {
                TextView label = reopened.findViewById(R.id.ui_venue_selected);
                checks.check(label != null && !label.getText().toString().contains(preset.name)
                                && reopened.getWindow().getDecorView().findViewWithTag("venue_preset_" + preset.id) == null,
                        "Reopening the venue manager removes the deleted preset from its current selection and list");
            });
        } finally {
            if (activity != null) {
                final Activity opened = activity;
                instrumentation.runOnMainSync(opened::finish);
                instrumentation.waitForIdleSync();
            }
            watches.remove(watch.id);
            venues.removeVenue(venueKey);
        }
        return checks.assertions;
    }

    private static void setDialogText(Instrumentation instrumentation, String id, String text) {
        Bundle arguments = new Bundle();
        arguments.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text);
        performAccessibilityAction(instrumentation, "app.ticketwatch.mobile:id/" + id,
                AccessibilityNodeInfo.ACTION_SET_TEXT, arguments);
        instrumentation.waitForIdleSync();
    }

    private static void performAccessibilityAction(Instrumentation instrumentation, String id, int action, Bundle arguments) {
        long deadline = SystemClock.uptimeMillis() + 5000;
        do {
            AccessibilityNodeInfo root = instrumentation.getUiAutomation().getRootInActiveWindow();
            if (root != null) {
                try { if (tryAccessibilityAction(root, id, action, arguments)) return; }
                finally { root.recycle(); }
            }
            List<AccessibilityWindowInfo> windows = instrumentation.getUiAutomation().getWindows();
            try {
                for (AccessibilityWindowInfo window : windows) {
                    if (!window.isActive() && !window.isFocused()) continue;
                    AccessibilityNodeInfo windowRoot = window.getRoot();
                    if (windowRoot == null) continue;
                    try { if (tryAccessibilityAction(windowRoot, id, action, arguments)) return; }
                    finally { windowRoot.recycle(); }
                }
            } finally {
                for (AccessibilityWindowInfo window : windows) window.recycle();
            }
            SystemClock.sleep(75);
        } while (SystemClock.uptimeMillis() < deadline);
        throw new AssertionError("Venue dialog control did not become ready for " + id);
    }

    private static boolean tryAccessibilityAction(AccessibilityNodeInfo root, String id, int action, Bundle arguments) {
        if (!"app.ticketwatch.mobile".equals(String.valueOf(root.getPackageName()))) return false;
        List<AccessibilityNodeInfo> nodes = root.findAccessibilityNodeInfosByViewId(id);
        try {
            if (nodes.size() != 1 || !nodes.get(0).isEnabled()
                    || (action == AccessibilityNodeInfo.ACTION_CLICK && !nodes.get(0).isVisibleToUser())) return false;
            // Poll only readiness. Never retry an action after dispatching it.
            if (!nodes.get(0).performAction(action, arguments))
                throw new AssertionError("Venue dialog action failed for " + id);
            return true;
        } finally {
            for (AccessibilityNodeInfo node : nodes) node.recycle();
        }
    }

    private static void savePreview(Activity activity) {
        try {
            View root = activity.getWindow().getDecorView();
            Bitmap bitmap = Bitmap.createBitmap(root.getWidth(), root.getHeight(), Bitmap.Config.ARGB_8888);
            root.draw(new Canvas(bitmap));
            File directory = activity.getExternalFilesDir("ui-previews");
            if (directory == null || (!directory.exists() && !directory.mkdirs())) throw new IllegalStateException();
            try (FileOutputStream file = new FileOutputStream(new File(directory, "android-venues.png"))) {
                if (!bitmap.compress(Bitmap.CompressFormat.PNG, 100, file)) throw new IllegalStateException();
            } finally { bitmap.recycle(); }
        } catch (Exception failure) { throw new AssertionError("Synthetic venue preview could not be saved", failure); }
    }
}
