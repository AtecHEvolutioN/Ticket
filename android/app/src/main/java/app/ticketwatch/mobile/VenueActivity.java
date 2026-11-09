package app.ticketwatch.mobile;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.os.Bundle;
import android.text.InputFilter;
import android.text.InputType;
import android.view.Gravity;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.EditText;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.util.ArrayList;
import java.util.List;

/** Reusable venue references. Every event still needs its own live ticket observation. */
public final class VenueActivity extends Activity {
    private static final int BG = Color.rgb(9, 17, 29);
    private static final int CARD = Color.rgb(18, 31, 47);
    private static final int FIELD = Color.rgb(10, 23, 38);
    private static final int LINE = Color.rgb(38, 57, 76);
    private static final int TEXT = Color.rgb(237, 244, 250);
    private static final int MUTED = Color.rgb(153, 174, 192);
    private static final int TEAL = Color.rgb(89, 229, 202);
    private WatchStore watches;
    private VenueStore venues;
    private String watchId = "";
    private String venueKey = "";
    private LinearLayout body;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        requestWindowFeature(Window.FEATURE_NO_TITLE);
        watches = new WatchStore(this);
        venues = new VenueStore(this);
        watchId = value(getIntent().getStringExtra("watch_id"));
        venueKey = state == null ? value(getIntent().getStringExtra("venue_key"))
                : value(state.getString("venue_key"));
        getWindow().setStatusBarColor(BG);
        getWindow().setNavigationBarColor(BG);
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        if (Build.VERSION.SDK_INT >= 30) getWindow().setDecorFitsSystemWindows(false);
        else getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        for (Models.Watch saved : watches.all()) {
            try { venues.register(saved); }
            catch (RuntimeException ignored) { /* The saved watch remains usable without a venue entry. */ }
        }
        render();
    }

    @Override protected void onResume() {
        super.onResume();
        if (body != null) render();
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        state.putString("venue_key", venueKey);
        super.onSaveInstanceState(state);
    }

    @Override public void onBackPressed() {
        if (watchId.isEmpty() && !venueKey.isEmpty()) { venueKey = ""; render(); }
        else super.onBackPressed();
    }

    private void render() {
        LinearLayout root = column();
        root.setBackgroundColor(BG);
        root.setOnApplyWindowInsetsListener((view, insets) -> {
            view.setPadding(insets.getSystemWindowInsetLeft(), insets.getSystemWindowInsetTop(),
                    insets.getSystemWindowInsetRight(), insets.getSystemWindowInsetBottom());
            return insets;
        });
        LinearLayout heading = new LinearLayout(this);
        heading.setGravity(Gravity.CENTER_VERTICAL);
        heading.setPadding(dp(8), dp(4), dp(20), dp(4));
        ImageButton back = new ImageButton(this);
        back.setId(R.id.ui_back);
        back.setImageResource(R.drawable.ic_back);
        back.setImageTintList(ColorStateList.valueOf(TEXT));
        back.setBackground(surface(BG, BG));
        back.setPadding(dp(13), dp(13), dp(13), dp(13));
        back.setContentDescription(watchId.isEmpty() ? (venueKey.isEmpty() ? "Back to watchlist" : "Back to venues") : "Back to event");
        back.setOnClickListener(view -> onBackPressed());
        heading.addView(back, new LinearLayout.LayoutParams(dp(48), dp(48)));
        TextView title = text("Venue presets", 18, TEXT, true);
        title.setPadding(dp(12), 0, 0, 0);
        heading.addView(title, new LinearLayout.LayoutParams(0, -2, 1));
        root.addView(heading);
        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        scroll.setVerticalScrollBarEnabled(false);
        body = column();
        body.setPadding(dp(20), dp(12), dp(20), dp(28));
        scroll.addView(body, new ScrollView.LayoutParams(-1, -2));
        root.addView(scroll, new LinearLayout.LayoutParams(-1, 0, 1));
        setContentView(root);
        root.requestApplyInsets();
        if (!watchId.isEmpty()) {
            Models.Watch watch = watches.get(watchId);
            if (watch == null) { empty("Event no longer saved", "Return to your watchlist to choose another event."); return; }
            VenueStore.Venue venue = venues.forWatch(watch);
            if (venue == null) {
                empty("Venue match unavailable", "This event needs an official venue ID, or an exact venue name, city and country that match the saved venue. You can still check this event normally.");
                return;
            }
            venueKey = venue.key;
            detail(venue, watch);
        } else if (!venueKey.isEmpty()) {
            VenueStore.Venue venue = null;
            for (VenueStore.Venue candidate : venues.all()) if (venueKey.equals(candidate.key)) venue = candidate;
            if (venue == null) empty("Venue no longer saved", "Go back to see the saved venues.");
            else detail(venue, null);
        } else list();
    }

    private void list() {
        body.addView(text("Know the venue", 26, TEXT, true));
        paragraph(body, "Save named section references and notes, then choose them for another event at the same venue.");
        List<VenueStore.Venue> saved = venues.all();
        if (saved.isEmpty()) {
            empty("No venues saved yet", "Save an event with venue details from Explore. Its venue will appear here.");
            return;
        }
        for (VenueStore.Venue venue : saved) {
            LinearLayout card = card();
            card.addView(text(value(venue.name).isEmpty() ? "Saved venue" : venue.name, 19, TEXT, true));
            paragraph(card, location(venue) + "\n" + venue.site);
            int count = venues.presetsForVenue(venue.key).size();
            card.addView(text(count + (count == 1 ? " preset" : " presets"), 12, TEAL, false));
            button(card, "View presets", false, () -> { venueKey = venue.key; render(); })
                    .setTag("venue_open_" + venue.key);
            add(body, card, 16);
        }
    }

    private void detail(VenueStore.Venue venue, Models.Watch scopedWatch) {
        body.addView(text(value(venue.name).isEmpty() ? "Saved venue" : venue.name, 25, TEXT, true));
        paragraph(body, location(venue) + "\n" + venue.site);
        body.addView(text(value(venue.venueId).isEmpty() ? "Matched by exact venue name and location"
                : "Matched by Ticketmaster venue ID", 12, TEAL, false));
        paragraph(body, "Section references are saved notes, not a seating map. Each event checks its own ticket types, prices and availability.");
        if (scopedWatch != null) {
            LinearLayout selected = card();
            selected.addView(text("FOR THIS EVENT", 11, TEAL, true));
            add(selected, text(scopedWatch.name, 17, TEXT, true), 7);
            VenueStore.Preset preset = venues.selected(scopedWatch);
            TextView current = text(preset == null ? "No venue preset selected" : "Using: " + preset.name, 13, TEXT, false);
            current.setId(R.id.ui_venue_selected);
            add(selected, current, 8);
            if (preset != null) {
                paragraph(selected, sections(preset));
                button(selected, "Clear for this event", false, () -> clearSelection(scopedWatch.id));
            }
            add(body, selected, 16);
        }
        Models.Watch writer = scopedWatch == null ? matchingWatch(venue.key) : scopedWatch;
        if (writer != null) button(body, "Add preset", true, () -> addPreset(writer.id)).setId(R.id.ui_venue_add);
        else paragraph(body, "Save an event at this venue to add a new preset. Existing references remain here after removing a watch.");
        List<VenueStore.Preset> presets = venues.presetsForVenue(venue.key);
        if (presets.isEmpty()) {
            paragraph(body, "No presets yet. For example, name a concert configuration and add the section labels you want to remember.");
        }
        VenueStore.Preset selected = scopedWatch == null ? null : venues.selected(scopedWatch);
        for (VenueStore.Preset preset : presets) {
            LinearLayout card = card();
            card.setTag("venue_preset_" + preset.id);
            card.addView(text(preset.name, 19, TEXT, true));
            paragraph(card, sections(preset));
            if (!value(preset.notes).isEmpty()) paragraph(card, preset.notes);
            if (scopedWatch != null) {
                boolean isSelected = selected != null && preset.id.equals(selected.id);
                Button use = button(card, isSelected ? "Selected for this event" : "Use for this event", !isSelected, () -> {
                    try {
                        if (venues.apply(watches, scopedWatch.id, preset.id)) { toast("Venue reference selected."); render(); }
                        else { toast("This preset no longer matches the event's venue."); render(); }
                    } catch (RuntimeException error) { toast("Could not save the selection. Try again."); }
                });
                use.setTag("venue_use_" + preset.id);
                use.setEnabled(!isSelected);
            }
            button(card, "Remove preset", false, () -> new AlertDialog.Builder(this)
                    .setTitle("Remove this venue preset?")
                    .setMessage("Remove “" + preset.name + "” from saved references? Events will keep their own ticket observations.")
                    .setNegativeButton("Cancel", null)
                    .setPositiveButton("Remove", (dialog, which) -> {
                        try { venues.removePreset(preset.id); render(); }
                        catch (RuntimeException error) { toast("Could not remove this preset. Try again."); }
                    }).show()).setTag("venue_remove_" + preset.id);
            add(body, card, 16);
        }
    }

    private Models.Watch matchingWatch(String key) {
        for (Models.Watch watch : watches.all()) {
            VenueStore.Venue venue = venues.forWatch(watch);
            if (venue != null && key.equals(venue.key)) return watch;
        }
        return null;
    }

    private void clearSelection(String id) {
        try {
            if (!venues.apply(watches, id, "")) toast("This event is no longer saved.");
            render();
        }
        catch (RuntimeException error) { toast("Could not clear this selection. Try again."); }
    }

    private void addPreset(String id) {
        LinearLayout fields = column();
        fields.setPadding(dp(20), dp(4), dp(20), dp(16));
        fields.setBackgroundColor(BG);
        EditText name = field(fields, "Preset name", "e.g. Concert configuration", 80, false);
        name.setId(R.id.ui_venue_preset_name);
        EditText references = field(fields, "Section references · one per line", "e.g. Lower level A", 4050, true);
        references.setId(R.id.ui_venue_sections);
        EditText notes = field(fields, "Notes · optional", "Static venue notes", 1000, true);
        notes.setId(R.id.ui_venue_notes);
        paragraph(fields, "Save only reference labels and venue notes. Ticket offers and availability belong to each event.");
        ScrollView scroll = new ScrollView(this);
        scroll.addView(fields);
        AlertDialog dialog = new AlertDialog.Builder(this).setTitle("Add venue preset")
                .setView(scroll).setNegativeButton("Cancel", null).setPositiveButton("Save", null).create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            String label = name.getText().toString().trim();
            if (label.isEmpty()) { name.setError("Give this preset a name."); return; }
            Models.Watch current = watches.get(id);
            VenueStore.Venue currentVenue = current == null ? null : venues.forWatch(current);
            if (currentVenue == null || !venueKey.equals(currentVenue.key)) {
                toast("The saved event's venue has changed. Open its venue again."); dialog.dismiss(); render(); return;
            }
            List<String> sections = new ArrayList<>();
            for (String line : references.getText().toString().split("\\r?\\n")) {
                String section = line.trim();
                if (section.length() > 80) { references.setError("Keep each section reference within 80 characters."); return; }
                if (!section.isEmpty() && !sections.contains(section)) sections.add(section);
            }
            if (sections.size() > 50) { references.setError("Save up to 50 section references per preset."); return; }
            try {
                VenueStore.Preset saved = venues.savePreset(current, label, sections, notes.getText().toString().trim());
                if (saved == null) { toast("Could not save a preset for this venue."); return; }
                dialog.dismiss(); render(); toast("Venue preset saved.");
            } catch (IllegalArgumentException error) { toast("Check the preset name, section references and notes."); }
            catch (RuntimeException error) { toast("Could not save this preset. Try again."); }
        }));
        dialog.show();
    }

    private void empty(String title, String message) {
        LinearLayout card = card();
        card.addView(text(title, 20, TEXT, true));
        paragraph(card, message);
        add(body, card, 16);
    }

    private EditText field(LinearLayout parent, String label, String hint, int limit, boolean multiline) {
        add(parent, text(label, 12, MUTED, true), 14);
        EditText input = new EditText(this);
        input.setTextColor(TEXT);
        input.setHintTextColor(MUTED);
        input.setTextSize(14);
        input.setHint(hint);
        input.setContentDescription(label);
        input.setBackground(surface(FIELD, LINE));
        input.setPadding(dp(12), dp(12), dp(12), dp(12));
        input.setMinimumHeight(dp(48));
        input.setFilters(new InputFilter[]{new InputFilter.LengthFilter(limit)});
        input.setInputType(InputType.TYPE_CLASS_TEXT | (multiline ? InputType.TYPE_TEXT_FLAG_MULTI_LINE : InputType.TYPE_TEXT_FLAG_CAP_SENTENCES));
        input.setSingleLine(!multiline);
        if (multiline) { input.setMinLines(2); input.setGravity(Gravity.TOP | Gravity.START); }
        add(parent, input, 6);
        return input;
    }

    private Button button(LinearLayout parent, String label, boolean primary, Runnable click) {
        Button button = new Button(this);
        button.setText(label);
        button.setAllCaps(false);
        button.setTextSize(14);
        button.setTypeface(null, Typeface.BOLD);
        button.setTextColor(primary ? BG : TEXT);
        button.setBackground(surface(primary ? TEAL : CARD, primary ? TEAL : LINE));
        button.setMinHeight(dp(48));
        button.setMinimumHeight(dp(48));
        button.setPadding(dp(12), dp(10), dp(12), dp(10));
        button.setOnClickListener(view -> click.run());
        add(parent, button, 12);
        return button;
    }

    private LinearLayout column() { LinearLayout result = new LinearLayout(this); result.setOrientation(LinearLayout.VERTICAL); return result; }
    private LinearLayout card() { LinearLayout result = column(); result.setPadding(dp(16), dp(16), dp(16), dp(16)); result.setBackground(surface(CARD, LINE)); return result; }
    private TextView text(String value, int size, int color, boolean bold) {
        TextView result = new TextView(this);
        result.setText(value); result.setTextSize(size); result.setTextColor(color);
        result.setLineSpacing(dp(3), 1);
        if (bold) result.setTypeface(null, Typeface.BOLD);
        return result;
    }
    private void paragraph(LinearLayout parent, String value) { add(parent, text(value, 13, MUTED, false), 9); }
    private void add(LinearLayout parent, View view, int top) { LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2); params.topMargin = dp(top); parent.addView(view, params); }
    private GradientDrawable surface(int fill, int stroke) { GradientDrawable result = new GradientDrawable(); result.setColor(fill); result.setCornerRadius(dp(14)); result.setStroke(dp(1), stroke); return result; }
    private int dp(int value) { return Math.round(getResources().getDisplayMetrics().density * value); }
    private void toast(String message) { Toast.makeText(this, message, Toast.LENGTH_LONG).show(); }
    private static String value(String text) { return text == null ? "" : text; }
    private static String location(VenueStore.Venue venue) {
        return value(venue.city).isEmpty() ? value(venue.country)
                : venue.city + (value(venue.country).isEmpty() ? "" : " · " + venue.country);
    }
    private static String sections(VenueStore.Preset preset) {
        return preset.sectionReferences == null || preset.sectionReferences.isEmpty() ? "No section references saved"
                : "Section references\n" + android.text.TextUtils.join(" · ", preset.sectionReferences);
    }
}
