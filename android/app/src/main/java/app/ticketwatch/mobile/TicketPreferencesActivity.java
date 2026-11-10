package app.ticketwatch.mobile;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.os.Bundle;
import android.text.InputFilter;
import android.text.InputType;
import android.text.Editable;
import android.text.TextWatcher;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

/** Local preference editor and portable export. It does not submit provider actions. */
public final class TicketPreferencesActivity extends Activity {
    private static final int EXPORT_REQUEST = 4107;
    private static final int BG = Color.rgb(9, 17, 29), CARD = Color.rgb(18, 31, 47);
    private static final int FIELD = Color.rgb(10, 23, 38), LINE = Color.rgb(38, 57, 76);
    private static final int TEXT = Color.rgb(237, 244, 250), MUTED = Color.rgb(153, 174, 192);
    private static final int TEAL = Color.rgb(89, 229, 202), AMBER = Color.rgb(255, 203, 118);
    private TicketPreferencesStore store;
    private Models.Watch watch;
    private String eventUrl = "", pendingExport = "";
    private LinearLayout fields;
    private EditText quantity, budget, currency, types, sections, rows, categories;
    private CheckBox adjacent, useDefaults;
    private TextView saveStatus;
    private Button save;
    private boolean initializing;
    private boolean dirty;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        requestWindowFeature(Window.FEATURE_NO_TITLE);
        store = new TicketPreferencesStore(this);
        String id = getIntent().getStringExtra("watch_id");
        if (id != null && !id.isEmpty()) {
            watch = new WatchStore(this).get(id);
            if (watch == null || !TicketPreferencesStore.supportsEvent(watch.url)) {
                toast("Choose a saved Ticketmaster event first."); finish(); return;
            }
            eventUrl = watch.url;
        }
        getWindow().setStatusBarColor(BG);
        getWindow().setNavigationBarColor(BG);
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        if (Build.VERSION.SDK_INT >= 30) getWindow().setDecorFitsSystemWindows(false);
        else getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        try {
            buildUi();
            if (state != null) restoreDraft(state);
        } catch (RuntimeException error) { toast(error.getMessage()); finish(); }
    }

    private void buildUi() {
        LinearLayout root = column();
        root.setBackgroundColor(BG);
        root.setOnApplyWindowInsetsListener((view, insets) -> {
            int bottom = insets.getSystemWindowInsetBottom();
            if (Build.VERSION.SDK_INT >= 30) bottom = Math.max(bottom, insets.getInsets(WindowInsets.Type.ime()).bottom);
            view.setPadding(insets.getSystemWindowInsetLeft(), insets.getSystemWindowInsetTop(),
                    insets.getSystemWindowInsetRight(), bottom);
            return insets;
        });
        LinearLayout heading = new LinearLayout(this);
        heading.setGravity(Gravity.CENTER_VERTICAL);
        heading.setPadding(dp(8), dp(4), dp(20), dp(4));
        ImageButton back = new ImageButton(this);
        back.setImageResource(R.drawable.ic_back);
        back.setImageTintList(ColorStateList.valueOf(TEXT));
        back.setBackground(surface(BG, BG));
        back.setContentDescription("Back");
        back.setPadding(dp(13), dp(13), dp(13), dp(13));
        back.setOnClickListener(view -> finish());
        heading.addView(back, new LinearLayout.LayoutParams(dp(48), dp(48)));
        TextView headingTitle = text("Ticket preferences", 18, TEXT, true);
        headingTitle.setPadding(dp(12), 0, 0, 0);
        heading.addView(headingTitle, new LinearLayout.LayoutParams(0, -2, 1));
        root.addView(heading);
        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        scroll.setVerticalScrollBarEnabled(false);
        LinearLayout body = column();
        body.setPadding(dp(20), dp(10), dp(20), dp(24));
        scroll.addView(body, new ScrollView.LayoutParams(-1, -2));
        root.addView(scroll, new LinearLayout.LayoutParams(-1, 0, 1));
        body.addView(text(watch == null ? "YOUR DEFAULTS" : "FOR THIS EVENT", 11, TEAL, true));
        add(body, text(watch == null ? "Your tickets, your choices" : watch.name, 25, TEXT, true), 8);
        paragraph(body, watch == null ? "Set your usual quantity and price limit. Each event can use its own choices."
                : eventLocation() + "\nCustom choices apply only to this event.");

        LinearLayout notice = card();
        notice.addView(text("Saved preferences only", 13, AMBER, true));
        paragraph(notice, "Automatic carting is not active in the Android app. Export these choices for the desktop planner. Existing availability alerts are not filtered by these settings.");
        add(body, notice, 16);
        if (watch != null) {
            useDefaults = checkbox("Use my default preferences", "pref_use_defaults");
            useDefaults.setChecked(!store.hasOverride(eventUrl));
            add(body, useDefaults, 12);
            Button editDefaults = button("Edit global defaults", false);
            editDefaults.setOnClickListener(view -> startActivity(new Intent(this, TicketPreferencesActivity.class)));
            add(body, editDefaults, 4);
        }

        fields = column();
        add(body, fields, 16);
        LinearLayout countCard = card();
        countCard.addView(text("Tickets", 18, TEXT, true));
        paragraph(countCard, "Default: 2. Choose 1–20; the event's own limit still applies.");
        LinearLayout quantityRow = new LinearLayout(this);
        quantityRow.setGravity(Gravity.CENTER_VERTICAL);
        Button less = button("−", false);
        less.setContentDescription("One fewer ticket");
        less.setOnClickListener(view -> adjustQuantity(-1));
        quantityRow.addView(less, new LinearLayout.LayoutParams(dp(52), dp(52)));
        quantity = edit("Ticket quantity", "pref_quantity", false);
        quantity.setRawInputType(InputType.TYPE_CLASS_NUMBER);
        quantity.setGravity(Gravity.CENTER);
        quantity.setTextSize(22);
        LinearLayout.LayoutParams amountParams = new LinearLayout.LayoutParams(0, dp(54), 1);
        amountParams.leftMargin = amountParams.rightMargin = dp(10);
        quantityRow.addView(quantity, amountParams);
        Button more = button("+", false);
        more.setContentDescription("One more ticket");
        more.setOnClickListener(view -> adjustQuantity(1));
        quantityRow.addView(more, new LinearLayout.LayoutParams(dp(52), dp(52)));
        add(countCard, quantityRow, 14);
        adjacent = checkbox("Keep seats adjacent", "pref_adjacent");
        add(countCard, adjacent, 10);
        paragraph(countCard, "When switched off, separate seats may qualify. The current desktop planner supports seated tickets only.");
        fields.addView(countCard);

        LinearLayout moneyCard = card();
        moneyCard.addView(text("Price", 18, TEXT, true));
        paragraph(moneyCard, "Always choose the cheapest eligible group with a known complete total.");
        budget = field(moneyCard, "Maximum total, including all fees", "No total limit", "pref_max_total", false);
        // Hint a decimal keyboard while retaining raw pasted text for strict validation.
        // A numeric KeyListener could turn "123,45" or "-10" into a different valid amount.
        budget.setRawInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL);
        currency = field(moneyCard, "Currency", "CZK", "pref_currency", false);
        currency.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS);
        currency.setFilters(new InputFilter[] { new InputFilter.LengthFilter(3) });
        paragraph(moneyCard, "Leave the budget blank for no limit. Currency is not converted. The current Czech planner accepts CZK.");
        add(fields, moneyCard, 16);

        LinearLayout filterCard = card();
        filterCard.addView(text("Ticket types & seats", 18, TEXT, true));
        paragraph(filterCard, "Leave a list blank to allow any. Use one exact label per line; capitalization and repeated spaces are ignored. These are filters, not guarantees of availability.");
        types = field(filterCard, "Allowed ticket types", "For example: Standard", "pref_ticket_types", true);
        List<String> observedTypes = observedTypes();
        if (!observedTypes.isEmpty()) {
            Button choose = button("Choose types from last page check", false);
            choose.setTag("pref_choose_types");
            choose.setOnClickListener(view -> chooseTypes(observedTypes));
            add(filterCard, choose, 8);
        }
        sections = field(filterCard, "Allowed sections", "Any section", "pref_sections", true);
        rows = field(filterCard, "Allowed rows", "Any row", "pref_rows", true);
        paragraph(filterCard, "Missing evidence cannot satisfy a filter. Restricted-view and accessibility suitability are not verified by the current planner.");
        add(fields, filterCard, 16);

        LinearLayout advanced = card();
        advanced.addView(text("Advanced category filter", 16, TEXT, true));
        paragraph(advanced, "Optional category IDs from the event's desktop catalog. IDs must match exactly and are specific to an event.");
        categories = field(advanced, "Allowed category IDs", "Any category ID", "pref_category_ids", true);
        add(fields, advanced, 16);
        if (watch == null) paragraph(body, "Default label filters apply across events. Use event overrides when section names or categories differ by venue.");
        Button export = button("Save & export preferences", false);
        export.setTag("pref_export");
        export.setOnClickListener(view -> export());
        add(body, export, 20);
        paragraph(body, "Exports your defaults and all event overrides as JSON. It contains public event links and preferences, with no API keys or account sessions.");

        LinearLayout footer = column();
        footer.setPadding(dp(20), dp(10), dp(20), dp(12));
        footer.setBackgroundColor(CARD);
        saveStatus = text("", 12, MUTED, false);
        saveStatus.setTag("pref_status");
        saveStatus.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        footer.addView(saveStatus);
        save = button(watch == null ? "Save defaults" : "Save for this event", true);
        save.setTag("pref_save");
        save.setOnClickListener(view -> saveCurrent());
        add(footer, save, 8);
        root.addView(footer);
        setContentView(root);
        root.requestApplyInsets();
        setValues(store.effective(eventUrl));
        savedStatus();
        if (useDefaults != null) {
            enableFields(!useDefaults.isChecked());
            useDefaults.setOnCheckedChangeListener((view, checked) -> {
                if (initializing) return;
                if (checked) setValues(store.defaults());
                enableFields(!checked);
                markDirty();
            });
        }
        TextWatcher changes = new TextWatcher() {
            public void beforeTextChanged(CharSequence value, int start, int count, int after) { }
            public void onTextChanged(CharSequence value, int start, int before, int count) { markDirty(); }
            public void afterTextChanged(Editable value) { }
        };
        for (EditText input : new EditText[] { quantity, budget, currency, types, sections, rows, categories })
            input.addTextChangedListener(changes);
        adjacent.setOnCheckedChangeListener((view, checked) -> markDirty());
    }

    private void setValues(TicketPreferences value) {
        boolean previous = initializing;
        initializing = true;
        quantity.setText(Integer.toString(value.quantity));
        budget.setText(value.budgetText()); currency.setText(value.currency);
        adjacent.setChecked(value.requireAdjacent);
        types.setText(android.text.TextUtils.join("\n", value.allowedTicketTypes));
        sections.setText(android.text.TextUtils.join("\n", value.allowedSections));
        rows.setText(android.text.TextUtils.join("\n", value.allowedRows));
        categories.setText(android.text.TextUtils.join("\n", value.allowedCategoryIds));
        initializing = previous;
    }

    private TicketPreferences formValue() {
        TicketPreferences value = new TicketPreferences();
        try { value.quantity = Integer.parseInt(quantity.getText().toString().trim()); }
        catch (NumberFormatException error) { throw new IllegalArgumentException("Choose between 1 and 20 tickets."); }
        value.currency = currency.getText().toString().trim().toUpperCase(Locale.ROOT);
        value.maxTotalMinor = TicketPreferences.parseBudget(budget.getText().toString(), value.currency);
        value.requireAdjacent = adjacent.isChecked();
        value.allowedTicketTypes = TicketPreferences.lines(types.getText().toString());
        value.allowedSections = TicketPreferences.lines(sections.getText().toString());
        value.allowedRows = TicketPreferences.lines(rows.getText().toString());
        value.allowedCategoryIds = TicketPreferences.lines(categories.getText().toString(), true);
        return value.checkedCopy();
    }

    private boolean saveCurrent() {
        try {
            if (watch != null && new WatchStore(this).get(watch.id) == null)
                throw new IllegalStateException("This event is no longer saved.");
            if (watch == null) store.saveDefaults(formValue());
            else if (useDefaults.isChecked()) store.clearOverride(eventUrl);
            else store.saveOverride(eventUrl, formValue());
            setValues(store.effective(eventUrl));
            dirty = false;
            savedStatus();
            toast(watch == null ? "Default preferences saved." : "Event preferences saved.");
            return true;
        } catch (RuntimeException error) {
            saveStatus.setText(error.getMessage()); saveStatus.setTextColor(AMBER);
            return false;
        }
    }

    private void savedStatus() {
        saveStatus.setTextColor(MUTED);
        saveStatus.setText(watch == null ? "Global defaults · " + store.defaults().summary()
                : (store.hasOverride(eventUrl) ? "Event override · " : "Using defaults · ") + store.effective(eventUrl).summary());
    }

    private void markDirty() {
        if (initializing) return;
        dirty = true;
        saveStatus.setTextColor(AMBER);
        saveStatus.setText("Unsaved changes · tap " + (watch == null ? "Save defaults" : "Save for this event"));
    }

    private void export() {
        if (!saveCurrent()) return;
        try {
            pendingExport = store.exportJson();
            Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType("application/json");
            intent.putExtra(Intent.EXTRA_TITLE, "ticket-preferences.json");
            startActivityForResult(intent, EXPORT_REQUEST);
        } catch (RuntimeException error) { pendingExport = ""; toast("No document picker is available to save the export."); }
    }

    @Override protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != EXPORT_REQUEST) return;
        if (resultCode == RESULT_OK && data != null && data.getData() != null && !pendingExport.isEmpty()) {
            try (OutputStream output = getContentResolver().openOutputStream(data.getData(), "w")) {
                if (output == null) throw new IllegalStateException();
                output.write(pendingExport.getBytes(StandardCharsets.UTF_8));
                toast("Preferences exported. Automatic carting remains inactive.");
            } catch (Exception error) { toast("Could not write the export. Your saved preferences are unchanged."); }
        }
        pendingExport = "";
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        if (quantity != null) {
            state.putString("quantity", quantity.getText().toString());
            state.putString("budget", budget.getText().toString());
            state.putString("currency", currency.getText().toString());
            state.putString("types", types.getText().toString());
            state.putString("sections", sections.getText().toString());
            state.putString("rows", rows.getText().toString());
            state.putString("categories", categories.getText().toString());
            state.putBoolean("adjacent", adjacent.isChecked());
            state.putBoolean("use_defaults", useDefaults != null && useDefaults.isChecked());
            state.putString("export", pendingExport);
            state.putBoolean("dirty", dirty);
        }
        super.onSaveInstanceState(state);
    }

    private void restoreDraft(Bundle state) {
        if (!state.containsKey("quantity")) return;
        initializing = true;
        quantity.setText(state.getString("quantity")); budget.setText(state.getString("budget"));
        currency.setText(state.getString("currency")); types.setText(state.getString("types"));
        sections.setText(state.getString("sections")); rows.setText(state.getString("rows"));
        categories.setText(state.getString("categories")); adjacent.setChecked(state.getBoolean("adjacent"));
        if (useDefaults != null) { useDefaults.setChecked(state.getBoolean("use_defaults")); enableFields(!useDefaults.isChecked()); }
        pendingExport = state.getString("export", "");
        initializing = false;
        dirty = state.getBoolean("dirty");
        if (dirty) markDirty();
    }

    @Override protected void onResume() {
        super.onResume();
        if (fields != null && useDefaults != null && useDefaults.isChecked()) {
            try { setValues(store.defaults()); if (dirty) markDirty(); else savedStatus(); }
            catch (RuntimeException error) { saveStatus.setText(error.getMessage()); }
        }
    }

    private void adjustQuantity(int delta) {
        int current;
        try { current = Integer.parseInt(quantity.getText().toString()); }
        catch (NumberFormatException error) { current = 2; }
        quantity.setText(Integer.toString(Math.max(1, Math.min(20, current + delta))));
    }

    private void enableFields(boolean enabled) { enableTree(fields, enabled); fields.setAlpha(enabled ? 1f : .5f); }
    private void enableTree(View view, boolean enabled) {
        view.setEnabled(enabled);
        if (view instanceof ViewGroup) for (int index = 0; index < ((ViewGroup) view).getChildCount(); index++)
            enableTree(((ViewGroup) view).getChildAt(index), enabled);
    }

    private List<String> observedTypes() {
        List<String> result = new ArrayList<>();
        if (watch == null || watch.ticketSnapshot == null) return result;
        for (TicketData.Offer offer : watch.ticketSnapshot.offers) addType(result, offer.ticketType);
        for (TicketData.Category category : watch.ticketSnapshot.categories) addType(result, category.ticketType);
        return result;
    }
    private void addType(List<String> values, String candidate) {
        if (candidate == null || candidate.trim().isEmpty()) return;
        for (String value : values) if (value.equalsIgnoreCase(candidate.trim())) return;
        values.add(candidate.trim());
    }
    private void chooseTypes(List<String> observed) {
        List<String> options = new ArrayList<>(observed);
        List<String> selected;
        try { selected = TicketPreferences.lines(types.getText().toString()); }
        catch (RuntimeException error) { toast(error.getMessage()); return; }
        for (String value : selected) addType(options, value);
        boolean[] checked = new boolean[options.size()];
        for (int index = 0; index < options.size(); index++)
            for (String chosen : selected) if (chosen.equalsIgnoreCase(options.get(index))) checked[index] = true;
        new AlertDialog.Builder(this).setTitle("Ticket type labels")
                .setMultiChoiceItems(options.toArray(new String[0]), checked, (dialog, which, value) -> checked[which] = value)
                .setNegativeButton("Cancel", null).setNeutralButton("Allow any", (dialog, which) -> types.setText(""))
                .setPositiveButton("Use selected", (dialog, which) -> {
                    List<String> values = new ArrayList<>();
                    for (int index = 0; index < options.size(); index++) if (checked[index]) values.add(options.get(index));
                    types.setText(android.text.TextUtils.join("\n", values));
                }).show();
    }

    private String eventLocation() {
        String result = Models.safe(watch.venue);
        if (!watch.city.isEmpty()) result += (result.isEmpty() ? "" : " · ") + watch.city;
        if (!watch.country.isEmpty()) result += (result.isEmpty() ? "" : " · ") + watch.country;
        return result;
    }
    private LinearLayout column() { LinearLayout result = new LinearLayout(this); result.setOrientation(LinearLayout.VERTICAL); return result; }
    private LinearLayout card() { LinearLayout result = column(); result.setPadding(dp(16), dp(16), dp(16), dp(16)); result.setBackground(surface(CARD, LINE)); return result; }
    private TextView text(String value, int size, int color, boolean bold) {
        TextView result = new TextView(this); result.setText(value); result.setTextSize(size); result.setTextColor(color);
        result.setLineSpacing(dp(3), 1); if (bold) result.setTypeface(null, Typeface.BOLD); return result;
    }
    private EditText edit(String label, String tag, boolean multiline) {
        EditText result = new EditText(this); result.setTextColor(TEXT); result.setHintTextColor(MUTED);
        result.setTextSize(14); result.setPadding(dp(13), dp(12), dp(13), dp(12));
        result.setBackground(surface(FIELD, LINE)); result.setMinimumHeight(dp(50));
        result.setContentDescription(label); result.setTag(tag); result.setSingleLine(!multiline);
        result.setInputType(InputType.TYPE_CLASS_TEXT | (multiline ? InputType.TYPE_TEXT_FLAG_MULTI_LINE : 0));
        result.setFilters(new InputFilter[] { new InputFilter.LengthFilter(multiline ? 13000 : 30) });
        if (multiline) { result.setMinLines(2); result.setMaxLines(5); result.setGravity(Gravity.TOP | Gravity.START); }
        return result;
    }
    private EditText field(LinearLayout parent, String label, String hint, String tag, boolean multiline) {
        add(parent, text(label, 12, TEXT, true), 16);
        EditText result = edit(label, tag, multiline); result.setHint(hint); add(parent, result, 7); return result;
    }
    private CheckBox checkbox(String label, String tag) {
        CheckBox result = new CheckBox(this); result.setText(label); result.setTextColor(TEXT); result.setTextSize(14);
        result.setButtonTintList(ColorStateList.valueOf(TEAL)); result.setMinHeight(dp(48)); result.setTag(tag); return result;
    }
    private Button button(String label, boolean primary) {
        Button result = new Button(this); result.setText(label); result.setAllCaps(false); result.setTextSize(14);
        result.setTextColor(primary ? BG : TEXT); result.setTypeface(null, Typeface.BOLD);
        result.setBackground(surface(primary ? TEAL : FIELD, primary ? TEAL : LINE));
        result.setPadding(dp(12), dp(10), dp(12), dp(10)); result.setMinHeight(dp(48)); return result;
    }
    private void paragraph(LinearLayout parent, String value) { add(parent, text(value, 13, MUTED, false), 9); }
    private void add(LinearLayout parent, View view, int top) { LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2); params.topMargin = dp(top); parent.addView(view, params); }
    private GradientDrawable surface(int fill, int stroke) { GradientDrawable result = new GradientDrawable(); result.setColor(fill); result.setCornerRadius(dp(12)); result.setStroke(dp(1), stroke); return result; }
    private int dp(int value) { return Math.round(getResources().getDisplayMetrics().density * value); }
    private void toast(String message) { Toast.makeText(this, message == null ? "Preferences could not be opened." : message, Toast.LENGTH_LONG).show(); }
}
