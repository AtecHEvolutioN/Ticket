package app.ticketwatch.mobile;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.text.InputType;
import android.text.Editable;
import android.text.TextWatcher;
import android.view.Gravity;
import android.view.View;
import android.view.WindowManager;
import android.view.WindowInsets;
import android.widget.Button;
import android.widget.EditText;
import android.widget.HorizontalScrollView;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import java.text.DateFormat;
import java.util.ArrayList;
import java.util.Date;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Small native client. Discovery metadata never claims ticket inventory. */
public final class MainActivity extends Activity {
    private static final int BG = Color.rgb(9, 17, 29);
    private static final int CARD = Color.rgb(18, 31, 47);
    private static final int FIELD = Color.rgb(10, 23, 38);
    private static final int LINE = Color.rgb(38, 57, 76);
    private static final int TEXT = Color.rgb(237, 244, 250);
    private static final int MUTED = Color.rgb(153, 174, 192);
    private static final int TEAL = Color.rgb(89, 229, 202);
    private static final int AMBER = Color.rgb(255, 203, 118);
    // Preserve the original synthetic demo key across app upgrades.
    private static final String DEMO_ID = "demo-korn-local";
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private final ArrayList<Models.Artist> artists = new ArrayList<>();
    private final ArrayList<Models.Event> events = new ArrayList<>();
    private SharedPreferences preferences;
    private SecureStore secrets;
    private WatchStore watches;
    private DiscoveryClient discovery;
    private LinearLayout shell;
    private LinearLayout body;
    private ScrollView scroller;
    private int renderedTab = -1;
    private EditText artistInput;
    private EditText countryInput;
    private EditText cityInput;
    private String query = "";
    private String country = "";
    private String city = "";
    private String selectedArtistId = "";
    private String selectedArtistName = "";
    private String selectedCountry = "";
    private String selectedCity = "";
    private String message = "";
    private String watchFilter = "all";
    private int tab = 0;
    private int page = -1;
    private int totalPages = 0;
    private int totalEvents = 0;
    private int generation = 0;
    private boolean loading;
    private boolean destroyed;
    private boolean discordTestRunning;
    private Button discordTestButton;
    private Models.Watch pendingNotification;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        preferences = getSharedPreferences("ticket_settings", MODE_PRIVATE);
        secrets = new SecureStore(this);
        watches = new WatchStore(this);
        discovery = new DiscoveryClient(this);
        query = preferences.getString("explore_query", "");
        country = preferences.getString("explore_country", "");
        city = preferences.getString("explore_city", "");
        if (state != null) tab = state.getInt("tab", 0);
        if (getIntent() != null && getIntent().hasExtra("watch_id")) tab = 1;
        getWindow().setStatusBarColor(BG);
        getWindow().setNavigationBarColor(BG);
        if (Build.VERSION.SDK_INT >= 30) getWindow().setDecorFitsSystemWindows(false);
        else getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        render();
    }

    @Override protected void onResume() {
        super.onResume();
        if (body != null && tab == 1) render();
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        saveSearchDraft();
        state.putInt("tab", tab);
        super.onSaveInstanceState(state);
    }

    @Override protected void onPause() {
        saveSearchDraft();
        super.onPause();
    }

    @Override protected void onDestroy() {
        destroyed = true;
        generation++;
        worker.shutdownNow();
        super.onDestroy();
    }

    private void render() {
        if (destroyed) return;
        final int previousScroll = renderedTab == tab && scroller != null ? scroller.getScrollY() : 0;
        renderedTab = tab;
        if (tab == 2) getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
        else getWindow().clearFlags(WindowManager.LayoutParams.FLAG_SECURE);
        artistInput = null;
        countryInput = null;
        cityInput = null;
        shell = column();
        shell.setBackgroundColor(BG);
        shell.setPadding(dp(16), dp(10), dp(16), 0);
        shell.setOnApplyWindowInsetsListener((view, insets) -> {
            int bottom = insets.getStableInsetBottom();
            if (Build.VERSION.SDK_INT >= 30) bottom = Math.max(bottom, insets.getInsets(WindowInsets.Type.ime()).bottom);
            else bottom = Math.max(bottom, insets.getSystemWindowInsetBottom());
            view.setPadding(dp(16), dp(10) + insets.getSystemWindowInsetTop(), dp(16), bottom);
            return insets;
        });
        setContentView(shell);
        shell.requestApplyInsets();

        LinearLayout brand = row();
        brand.setGravity(Gravity.CENTER_VERTICAL);
        TextView mark = text("t", 24, BG, true);
        mark.setGravity(Gravity.CENTER);
        mark.setBackground(shape(TEAL, 11, 0));
        mark.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        brand.addView(mark, new LinearLayout.LayoutParams(dp(36), dp(36)));
        TextView name = text("ticket watch", 18, TEXT, true);
        name.setPadding(dp(10), 0, dp(8), 0);
        brand.addView(name, new LinearLayout.LayoutParams(0, -2, 1));
        boolean keySaved = hasSecret("api_key");
        brand.addView(pill(keySaved ? "Key saved" : "Setup needed", keySaved ? TEAL : AMBER));
        shell.addView(brand);
        ScrollView scroll = new ScrollView(this);
        scroller = scroll;
        scroll.setFillViewport(true);
        scroll.setClipToPadding(false);
        scroll.setVerticalScrollBarEnabled(false);
        body = column();
        body.setPadding(0, dp(20), 0, dp(22));
        scroll.addView(body, new ScrollView.LayoutParams(-1, -2));
        LinearLayout.LayoutParams scrollParams = new LinearLayout.LayoutParams(-1, 0, 1);
        shell.addView(scroll, scrollParams);
        renderNavigation();
        if (tab == 0) renderExplore();
        else if (tab == 1) renderWatchlist();
        else renderSettings();
        scroll.post(() -> scroll.scrollTo(0, previousScroll));
    }

    private void switchTab(int nextTab) {
        saveSearchDraft();
        tab = nextTab;
        render();
    }

    private void renderNavigation() {
        LinearLayout navigation = row();
        navigation.setPadding(dp(4), dp(6), dp(4), dp(6));
        navigation.setBackground(shape(FIELD, 16, LINE));
        String[] labels = {"Explore", "Watchlist", "Settings"};
        int[] icons = {R.drawable.ic_explore, R.drawable.ic_watchlist, R.drawable.ic_settings};
        int[] ids = {R.id.ui_explore, R.id.ui_watchlist, R.id.ui_settings};
        for (int index = 0; index < labels.length; index++) {
            final int next = index;
            boolean selected = tab == index;
            LinearLayout item = column();
            item.setId(ids[index]);
            item.setGravity(Gravity.CENTER);
            item.setPadding(dp(4), dp(7), dp(4), dp(7));
            item.setMinimumHeight(dp(54));
            item.setSelected(selected);
            item.setFocusable(true);
            item.setContentDescription(labels[index] + (selected ? ", selected" : ""));
            if (selected) item.setBackground(shape(Color.argb(22, 89, 229, 202), 12, 0));
            ImageView icon = new ImageView(this);
            icon.setImageResource(icons[index]);
            icon.setColorFilter(selected ? TEAL : MUTED);
            icon.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
            item.addView(icon, new LinearLayout.LayoutParams(dp(21), dp(21)));
            gap(item, 4);
            TextView label = text(labels[index], 11, selected ? TEAL : MUTED, selected);
            label.setGravity(Gravity.CENTER);
            label.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
            item.addView(label);
            item.setOnClickListener(view -> switchTab(next));
            navigation.addView(item, new LinearLayout.LayoutParams(0, -2, 1));
        }
        shell.addView(navigation);
    }

    private void renderExplore() {
        title(body, "Find your next show");
        body.addView(text("Pick an artist. Choose where. Save your events.", 12, MUTED, false));
        LinearLayout quick = row();
        if (!hasSecret("api_key")) addRowButton(quick, "Add API key", false, () -> switchTab(2));
        Button demo = button("Try local demo", false, this::startDemo);
        demo.setId(R.id.ui_demo);
        LinearLayout.LayoutParams demoParams = new LinearLayout.LayoutParams(0, -2, 1);
        demoParams.topMargin = dp(12);
        quick.addView(demo, demoParams);
        body.addView(quick);
        gap(body, 14);

        LinearLayout search = card();
        artistInput = field(search, "Artist", "Artist or band name", query, false);
        artistInput.setTag("explore_artist");
        artistInput.setImeOptions(android.view.inputmethod.EditorInfo.IME_ACTION_SEARCH);
        artistInput.setOnEditorActionListener((view, actionId, event) -> {
            if (actionId == android.view.inputmethod.EditorInfo.IME_ACTION_SEARCH && !loading) { findArtists(); return true; }
            return false;
        });
        LinearLayout filters = row();
        LinearLayout countryBox = column();
        LinearLayout cityBox = column();
        countryInput = field(countryBox, "Country · optional", "Any country", country, false);
        countryInput.setTag("explore_country");
        countryInput.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS);
        countryInput.setFilters(new android.text.InputFilter[]{new android.text.InputFilter.LengthFilter(2), new android.text.InputFilter.AllCaps()});
        cityInput = field(cityBox, "City · optional", "Any city", city, false);
        cityInput.setTag("explore_city");
        LinearLayout.LayoutParams first = new LinearLayout.LayoutParams(0, -2, 1);
        first.rightMargin = dp(10);
        filters.addView(countryBox, first);
        filters.addView(cityBox, new LinearLayout.LayoutParams(0, -2, 1));
        search.addView(filters);
        renderCountryChips(search);
        Button find = action(search, loading ? "Searching…" : "Find artist", true, this::findArtists);
        find.setId(R.id.ui_search);
        find.setEnabled(!loading);
        body.addView(search);
        gap(body, 16);

        if (!message.isEmpty()) {
            TextView notice = text(message, 13, AMBER, false);
            notice.setPadding(dp(15), dp(13), dp(15), dp(13));
            notice.setBackground(shape(CARD, 12, LINE));
            body.addView(notice);
            gap(body, 14);
        }
        if (loading) {
            body.addView(text(selectedArtistId.isEmpty() ? "Finding matching artists…" : "Loading official event listings…", 13, TEAL, false));
            gap(body, 12);
        }
        if (!artists.isEmpty() && selectedArtistId.isEmpty()) {
            section(body, "Choose the artist", artists.size() + " matches");
            for (Models.Artist artist : artists) {
                LinearLayout artistCard = card();
                artistCard.addView(text(artist.name, 18, TEXT, true));
                artistCard.addView(text("Official Ticketmaster artist", 11, MUTED, false));
                Button pick = action(artistCard, "View events", false, () -> chooseArtist(artist));
                pick.setEnabled(!loading);
                body.addView(artistCard);
                gap(body, 12);
            }
        }
        if (!selectedArtistId.isEmpty()) {
            section(body, selectedArtistName + " events", totalEvents + " found");
            String location = selectedCountry.isEmpty() ? "All countries" : selectedCountry;
            if (!selectedCity.isEmpty()) location += " · " + selectedCity;
            body.addView(text(location + " · Listings do not confirm live availability.", 12, MUTED, false));
            gap(body, 13);
            for (Models.Event event : events) eventCard(event);
            if (events.isEmpty() && !loading && message.isEmpty()) paragraph(body, "No events match these filters. Try another country or leave the city empty.");
            if (page + 1 < totalPages) {
                Button more = action(body, loading ? "Loading…" : "Load more events", false, () -> loadEvents(page + 1));
                more.setEnabled(!loading);
                TextView count = text(events.size() + " of " + totalEvents + " events · 50 per page", 11, MUTED, false);
                count.setGravity(Gravity.CENTER);
                body.addView(count);
            }
            gap(body, 19);
        }
        LinearLayout manual = card();
        manual.addView(text("Have an event link?", 17, TEXT, true));
        paragraph(manual, "Save a Ticketmaster event directly. No API key needed.");
        Button add = action(manual, "Add event URL", false, this::showManualDialog);
        add.setId(R.id.ui_add_watch);
        body.addView(manual);
        footer(body);
    }

    private void renderCountryChips(LinearLayout parent) {
        HorizontalScrollView scroll = new HorizontalScrollView(this);
        scroll.setHorizontalScrollBarEnabled(false);
        LinearLayout chips = row();
        String[] codes = {"", "PL", "CZ", "DE", "GB", "US"};
        for (String code : codes) {
            TextView chip = chip(code.isEmpty() ? "All" : code, code.equals(countryInput.getText().toString()), () -> {
                countryInput.setText(code);
                saveSearchDraft();
            });
            chip.setTag(code);
            chip.setContentDescription(code.isEmpty() ? "All countries" : "Country " + code);
            LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(dp(48), dp(48));
            params.rightMargin = dp(4);
            chips.addView(chip, params);
        }
        countryInput.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence value, int start, int count, int after) {}
            @Override public void onTextChanged(CharSequence value, int start, int before, int count) {
                for (int i = 0; i < chips.getChildCount(); i++) {
                    TextView chip = (TextView) chips.getChildAt(i);
                    styleChip(chip, chip.getTag().equals(value.toString()));
                }
            }
            @Override public void afterTextChanged(Editable value) {}
        });
        scroll.addView(chips);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
        params.topMargin = dp(8);
        parent.addView(scroll, params);
    }

    private void eventCard(Models.Event event) {
        LinearLayout card = card();
        boolean external = isExternalSeller(event.url);
        boolean supportedLink = !DiscoveryClient.normalizePublicUrl(event.url).isEmpty();
        card.addView(pill(!supportedLink ? "CATALOG DETAILS" : external ? "EXTERNAL SELLER" : "OFFICIAL EVENT", external || !supportedLink ? AMBER : TEAL));
        gap(card, 11);
        card.addView(text(nonempty(event.name, "Untitled event"), 19, TEXT, true));
        gap(card, 8);
        card.addView(text(eventMeta(event.date, event.city, event.country), 13, TEXT, false));
        if (!blank(event.venue)) card.addView(text(event.venue, 12, MUTED, false));
        gap(card, 10);
        card.addView(text("Sale-status metadata: " + nonempty(event.status, "unknown"), 11, MUTED, false));
        TicketViews.addCatalog(this, card, event.priceRanges);
        if (!supportedLink) {
            paragraph(card, "Discovery returned this event without a supported public ticket link. Page checking is unavailable here; this does not indicate sold-out tickets.");
            body.addView(card);
            gap(body, 12);
            return;
        }
        if (external) paragraph(card, "This event uses an external seller such as Front Gate. Automated availability checking is unsupported; open the official link to review tickets.");
        LinearLayout actions = row();
        addRowButton(actions, "Save watch", true, () -> saveEvent(event));
        addRowButton(actions, "Open ↗", false, () -> openUrl(event.url));
        card.addView(actions);
        body.addView(card);
        gap(body, 12);
    }

    private void findArtists() {
        saveSearchDraft();
        if (query.trim().isEmpty()) { toast("Enter an artist name."); return; }
        if (!country.isEmpty() && !country.matches("[A-Z]{2}")) { toast("Use a two-letter country code, such as PL."); return; }
        final String key = readSecret("api_key");
        if (key.isEmpty()) { switchTab(2); toast("Add your Discovery API key first."); return; }
        final String requestedQuery = query.trim();
        selectedArtistId = "";
        selectedArtistName = "";
        artists.clear();
        events.clear();
        message = "";
        page = -1;
        totalPages = 0;
        totalEvents = 0;
        loading = true;
        final int request = ++generation;
        render();
        worker.execute(() -> {
            try {
                List<Models.Artist> found = discovery.searchArtists(key, requestedQuery);
                deliver(request, () -> {
                    artists.addAll(found);
                    loading = false;
                    if (artists.isEmpty()) message = "No matching artists. Try a different spelling.";
                    renderDiscoveryResult();
                });
            } catch (Exception error) {
                deliver(request, () -> { loading = false; message = friendlyError(error); renderDiscoveryResult(); });
            }
        });
    }

    private void chooseArtist(Models.Artist artist) {
        saveSearchDraft();
        selectedArtistId = artist.id;
        selectedArtistName = artist.name;
        selectedCountry = country;
        selectedCity = city;
        events.clear();
        page = -1;
        totalEvents = 0;
        totalPages = 0;
        loadEvents(0);
    }

    private void loadEvents(int nextPage) {
        final String key = readSecret("api_key");
        if (key.isEmpty()) { toast("Add your Discovery API key in Settings."); return; }
        final String artistId = selectedArtistId;
        final String requestedCountry = selectedCountry;
        final String requestedCity = selectedCity;
        final int request = ++generation;
        loading = true;
        message = "";
        render();
        worker.execute(() -> {
            try {
                Models.EventPage result = discovery.events(key, artistId, requestedCountry, requestedCity, nextPage);
                deliver(request, () -> {
                    for (Models.Event event : result.events) {
                        boolean seen = false;
                        for (Models.Event prior : events) if (prior.id.equals(event.id)) { seen = true; break; }
                        if (!seen) events.add(event);
                    }
                    page = result.page;
                    totalPages = result.totalPages;
                    totalEvents = result.totalElements;
                    loading = false;
                    renderDiscoveryResult();
                });
            } catch (Exception error) {
                deliver(request, () -> { loading = false; message = friendlyError(error); renderDiscoveryResult(); });
            }
        });
    }

    private void saveEvent(Models.Event event) {
        final Models.Watch prepared;
        try { prepared = Models.Watch.fromEvent(event, selectedArtistName); }
        catch (IllegalArgumentException error) { toast("This event does not have a supported official event link."); return; }
        if (watches.get(prepared.id) != null) { toast("Already saved in your Watchlist."); return; }
        final EditText name = dialogField("Watch name", nonempty(event.name, "My event"), false);
        new AlertDialog.Builder(this).setTitle("Save a watch")
            .setMessage("Give this event a name you will recognize in alerts.")
            .setView(dialogWrap(name))
            .setNegativeButton("Cancel", null)
            .setPositiveButton("Save", (dialog, which) -> {
                prepared.name = nonempty(name.getText().toString().trim(), event.name);
                try {
                    if (watches.get(prepared.id) != null) { toast("Already saved in your Watchlist."); return; }
                    watches.save(prepared);
                    toast("Saved to Watchlist.");
                } catch (Exception error) { toast("Could not save this watch. Your watchlist may be full."); }
            }).show();
    }

    private void renderWatchlist() {
        List<Models.Watch> saved = watches.all();
        title(body, "Your watchlist");
        body.addView(text("Saved shows. Check a page for the latest status.", 12, MUTED, false));
        Button add = action(body, "Add event URL", true, this::showManualDialog);
        add.setId(R.id.ui_add_watch);
        action(body, "Venue presets", false, () -> startActivity(new Intent(this, VenueActivity.class)))
                .setId(R.id.ui_venue_presets);
        gap(body, 16);
        int available = 0;
        int real = 0;
        for (Models.Watch watch : saved) {
            if (watch.demo) continue;
            real++;
            if ("AVAILABLE".equals(watch.state)) available++;
        }
        LinearLayout stats = row();
        stat(stats, Integer.toString(real), "SAVED EVENTS");
        stat(stats, Integer.toString(available), "AVAILABLE");
        body.addView(stats);
        gap(body, 14);
        LinearLayout filters = row();
        String[] values = {"all", "available", "unchecked"};
        String[] labels = {"All", "Available", "Needs check"};
        for (int i = 0; i < labels.length; i++) {
            final String filter = values[i];
            TextView chip = chip(labels[i], filter.equals(watchFilter), () -> { watchFilter = filter; render(); });
            chip.setTag("watch_filter_" + filter);
            LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(0, dp(48), 1);
            if (i < labels.length - 1) params.rightMargin = dp(6);
            filters.addView(chip, params);
        }
        body.addView(filters);
        gap(body, 14);
        int visible = 0;
        for (Models.Watch watch : saved) {
            boolean match = "all".equals(watchFilter)
                || (!watch.demo && "available".equals(watchFilter) && "AVAILABLE".equals(watch.state))
                || (!watch.demo && "unchecked".equals(watchFilter) && (watch.checkedAt == 0 || "UNKNOWN".equals(watch.state) || "BLOCKED".equals(watch.state)));
            if (match) { watchCard(watch); visible++; }
        }
        if (visible == 0) {
            LinearLayout empty = card();
            empty.addView(text(saved.isEmpty() ? "Your next show starts here" : "No events in this view", 18, TEXT, true));
            paragraph(empty, saved.isEmpty() ? "Find an artist, paste an event link, or try a synthetic demo." : "Choose All to see your saved shows. Demo events are excluded from availability filters.");
            if (saved.isEmpty()) {
                action(empty, "Explore events", true, () -> switchTab(0));
                Button demo = action(empty, "Try local demo", false, this::startDemo);
                demo.setId(R.id.ui_demo);
            }
            body.addView(empty);
        }
        footer(body);
    }

    private void watchCard(Models.Watch watch) {
        LinearLayout card = card();
        String currentState = nonempty(watch.state, "UNKNOWN");
        String label = watch.demo ? "DEMO · " + stateLabel(currentState) : stateLabel(currentState);
        card.addView(pill(label.toUpperCase(Locale.ROOT), stateColor(currentState)));
        gap(card, 12);
        card.addView(text(watch.name, 20, TEXT, true));
        gap(card, 8);
        card.addView(text(eventMeta(watch.date, watch.city, watch.country), 13, TEXT, false));
        if (!blank(watch.venue)) card.addView(text(watch.venue, 12, MUTED, false));
        if (!watch.demo) {
            VenueStore.Preset preset = new VenueStore(this).selected(watch);
            if (preset != null) {
                paragraph(card, "Venue reference: " + preset.name);
                if (preset.sectionReferences != null && !preset.sectionReferences.isEmpty())
                    card.addView(text("Sections: " + android.text.TextUtils.join(" · ", preset.sectionReferences), 12, MUTED, false));
            }
        }
        gap(card, 11);
        card.addView(text(watch.checkedAt > 0 ? "Checked " + DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(new Date(watch.checkedAt)) : "Not checked yet", 11, MUTED, false));
        if (!blank(watch.detail)) paragraph(card, watch.detail);
        else paragraph(card, "Inventory is unknown until the page can be checked with verified evidence.");
        if (watch.priceRanges != null && !watch.priceRanges.isEmpty()) TicketViews.addCatalog(this, card, watch.priceRanges);
        TicketViews.addSnapshot(this, card, watch, true);
        if (watch.demo) {
            action(card, "Ticket details", false, () -> {
                Intent intent = new Intent(this, PageActivity.class);
                intent.putExtra("watch_id", watch.id);
                startActivity(intent);
            }).setId(R.id.ui_ticket_details);
            paragraph(card, "Synthetic event · local notifications only. These buttons do not check Ticketmaster or send Discord messages.");
            action(card, "1 · Simulate unavailable", false, () -> simulateDemo("UNAVAILABLE")).setId(R.id.ui_demo_unavailable);
            action(card, "2 · Simulate available", true, () -> simulateDemo("AVAILABLE")).setId(R.id.ui_demo_available);
            action(card, "Simulate blocked / unknown", false, () -> simulateDemo("BLOCKED")).setId(R.id.ui_demo_blocked);
        } else {
            boolean external = isExternalSeller(watch.url);
            if (external) paragraph(card, "External seller: availability checking is unsupported.");
            LinearLayout actions = row();
            addRowButton(actions, external ? "View details" : "Check page", true, () -> {
                Intent intent = new Intent(this, PageActivity.class);
                intent.putExtra("watch_id", watch.id);
                startActivity(intent);
            });
            addRowButton(actions, "Open ↗", false, () -> openUrl(watch.url));
            card.addView(actions);
            action(card, "Venue presets", false, () -> {
                Intent intent = new Intent(this, VenueActivity.class);
                intent.putExtra("watch_id", watch.id);
                startActivity(intent);
            }).setTag("watch_venue_" + watch.id);
        }
        TextView remove = text("Remove watch", 12, MUTED, false);
        remove.setPadding(0, dp(18), 0, dp(7));
        remove.setGravity(Gravity.CENTER);
        remove.setMinimumHeight(dp(48));
        remove.setFocusable(true);
        remove.setOnClickListener(view -> new AlertDialog.Builder(this)
            .setTitle("Remove this watch?").setMessage(watch.name)
            .setNegativeButton("Cancel", null).setPositiveButton("Remove", (dialog, which) -> {
                watches.remove(watch.id); render();
            }).show());
        card.addView(remove);
        body.addView(card);
        gap(body, 14);
    }

    private void renderSettings() {
        title(body, "Settings");
        paragraph(body, "Set up search and choose how you get alerts. Credentials are encrypted on this phone.");
        gap(body, 15);
        LinearLayout api = card();
        api.addView(text("1 · Event search", 19, TEXT, true));
        paragraph(api, "Add your Ticketmaster Consumer Key to find artists and events. Your Consumer Secret is not needed.");
        api.addView(pill(hasSecret("api_key") ? "KEY SAVED ON DEVICE" : "KEY NOT SET", hasSecret("api_key") ? TEAL : AMBER));
        EditText keyInput = field(api, "API key", hasSecret("api_key") ? "Enter a new key to replace the saved key" : "Paste your API key", "", true);
        keyInput.setTag("settings_api_key");
        action(api, "Save API key", true, () -> {
            String key = keyInput.getText().toString().trim();
            if (key.isEmpty()) { toast("Paste your API key first."); return; }
            try {
                secrets.put("api_key", key);
                keyInput.setText("");
                generation++;
                loading = false;
                artists.clear();
                events.clear();
                selectedArtistId = "";
                selectedArtistName = "";
                message = "";
                toast("API key saved securely.");
                render();
            } catch (Exception error) { toast("The key could not be saved securely. Please try again."); }
        });
        if (hasSecret("api_key")) action(api, "Remove saved key", false, () -> {
            try { secrets.remove("api_key"); generation++; loading = false; render(); toast("API key removed."); }
            catch (Exception error) { toast("Could not remove the key."); }
        });
        body.addView(api);
        gap(body, 15);

        LinearLayout notifications = card();
        notifications.addView(text("2 · Phone notifications", 19, TEXT, true));
        paragraph(notifications, "A first observation stays quiet. An alert fires only after unavailable → available. Blocked pages stay unknown and never prove availability.");
        action(notifications, "Test local notification", false, () -> {
            Models.Watch test = demoWatch();
            test.name = "Test notification · Ticket Watch";
            test.state = "AVAILABLE";
            notifyWithPermission(test);
        });
        body.addView(notifications);
        gap(body, 15);

        LinearLayout discord = card();
        discord.addView(text("3 · Discord · optional", 19, TEXT, true));
        paragraph(discord, "Send alerts through your own bot. Add its bot token, a channel it can post in, and the user ID to mention. The demo never sends to Discord.");
        EditText tokenInput = field(discord, "Bot token", hasSecret("discord_token") ? "Saved · enter a new token to replace it" : "Paste a bot token", "", true);
        EditText channelInput = field(discord, "Channel ID", "Discord channel ID", preferences.getString("discord_channel", ""), false);
        channelInput.setInputType(InputType.TYPE_CLASS_NUMBER);
        EditText userInput = field(discord, "User ID · optional", "User to mention", preferences.getString("discord_user", ""), false);
        userInput.setInputType(InputType.TYPE_CLASS_NUMBER);
        android.widget.Switch enabled = new android.widget.Switch(this);
        enabled.setText("Enable real-event Discord alerts");
        enabled.setTextColor(TEXT);
        enabled.setTextSize(13);
        enabled.setPadding(0, dp(10), 0, dp(12));
        enabled.setMinHeight(dp(48));
        enabled.setChecked(preferences.getBoolean("discord_enabled", false));
        discord.addView(enabled);
        action(discord, "Save Discord settings", false, () -> {
            String channel = channelInput.getText().toString().trim();
            String user = userInput.getText().toString().trim();
            String token = tokenInput.getText().toString().trim();
            if (!validDiscordIds(channel, user, enabled.isChecked())) return;
            if (enabled.isChecked() && token.isEmpty() && !hasSecret("discord_token")) { toast("Add a bot token to enable Discord alerts."); return; }
            try {
                if (!token.isEmpty()) secrets.put("discord_token", token);
                preferences.edit().putString("discord_channel", channel).putString("discord_user", user)
                    .putBoolean("discord_enabled", enabled.isChecked()).apply();
                tokenInput.setText("");
                toast("Discord settings saved.");
                render();
            } catch (Exception error) { toast("Discord settings could not be saved securely."); }
        });
        paragraph(discord, "The button below sends one clearly labeled test message to your saved Discord channel.");
        discordTestButton = action(discord, discordTestRunning ? "Sending test…" : "Send Discord test message", false, null);
        discordTestButton.setEnabled(!discordTestRunning);
        discordTestButton.setOnClickListener(view -> sendDiscordTest());
        if (hasSecret("discord_token")) action(discord, "Remove bot token", false, () -> {
            try {
                secrets.remove("discord_token");
                preferences.edit().putBoolean("discord_enabled", false).apply();
                toast("Bot token removed. Discord alerts disabled.");
                render();
            } catch (Exception error) { toast("Could not remove the bot token."); }
        });
        body.addView(discord);
        gap(body, 15);

        LinearLayout about = card();
        about.addView(text("About this test build", 17, TEXT, true));
        paragraph(about, "This testing app checks pages only while you use it. There is no scheduled or 24/7 monitoring. It does not bypass queues, CAPTCHAs, logins, or bot protection. It never buys tickets.");
        paragraph(about, "Ticketmaster Discovery provides event and sale-status metadata, not live ticket inventory. Front Gate and other external sellers can be opened but are not automatically checked.");
        body.addView(about);
        footer(body);
    }

    private void sendDiscordTest() {
        if (discordTestRunning) return;
        String token = readSecret("discord_token");
        String channel = preferences.getString("discord_channel", "");
        String user = preferences.getString("discord_user", "");
        if (token.isEmpty() || channel.isEmpty()) { toast("Save your bot token and channel ID first."); return; }
        discordTestRunning = true;
        discordTestButton.setEnabled(false);
        discordTestButton.setText("Sending test…");
        worker.execute(() -> {
            try {
                DiscordClient.sendTest(token, channel, user);
                runOnUiThread(() -> finishDiscordTest("Discord test message sent."));
            } catch (Exception error) {
                runOnUiThread(() -> finishDiscordTest("Discord test failed. Check the saved token, channel, and bot permissions."));
            }
        });
    }

    private void finishDiscordTest(String result) {
        discordTestRunning = false;
        if (destroyed) return;
        if (discordTestButton != null) {
            discordTestButton.setEnabled(true);
            discordTestButton.setText("Send Discord test message");
        }
        toast(result);
    }

    private boolean validDiscordIds(String channel, String user, boolean required) {
        if ((required && channel.isEmpty()) || (!channel.isEmpty() && !channel.matches("[0-9]{15,22}"))) {
            toast("Enter a valid numeric Discord channel ID."); return false;
        }
        if (!user.isEmpty() && !user.matches("[0-9]{15,22}")) { toast("Enter a valid numeric Discord user ID or leave it empty."); return false; }
        return true;
    }

    private void showManualDialog() {
        LinearLayout content = column();
        content.setPadding(dp(24), dp(8), dp(24), dp(8));
        EditText url = dialogField("Official Ticketmaster event URL", "", false);
        url.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        EditText name = dialogField("Watch name · optional", "", false);
        content.addView(url, new LinearLayout.LayoutParams(-1, -2));
        content.addView(name, new LinearLayout.LayoutParams(-1, -2));
        AlertDialog dialog = new AlertDialog.Builder(this).setTitle("Add an event URL")
            .setMessage("Save a public official event link. Availability starts as unknown.")
            .setView(content).setNegativeButton("Cancel", null).setPositiveButton("Save watch", null).create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            String safe = DiscoveryClient.normalizePublicUrl(url.getText().toString().trim());
            if (safe == null || safe.isEmpty()) { url.setError("Use a public HTTPS Ticketmaster or supported official seller URL."); return; }
            Models.Event event = new Models.Event();
            event.id = "manual-" + Integer.toHexString(safe.hashCode());
            event.name = nonempty(name.getText().toString().trim(), "My saved event");
            event.url = safe;
            event.status = "unknown";
            Models.Watch watch = Models.Watch.fromEvent(event, "");
            if (watches.get(watch.id) != null) { toast("Already saved in your Watchlist."); dialog.dismiss(); watchFilter = "all"; tab = 1; render(); return; }
            try { watches.save(watch); }
            catch (Exception error) { toast("Could not save this watch. Your watchlist may be full."); return; }
            dialog.dismiss();
            watchFilter = "all";
            tab = 1;
            render();
            toast("Event saved. Inventory is unknown.");
        }));
        dialog.show();
    }

    private Models.Watch demoWatch() {
        Models.Watch existing = watches.get(DEMO_ID);
        if (existing != null) { existing.name = "Demo concert"; existing.artist = "Demo artist"; return existing; }
        Models.Watch demo = new Models.Watch();
        demo.id = DEMO_ID;
        demo.name = "Demo concert";
        demo.artist = "Demo artist";
        demo.url = "";
        demo.date = "Synthetic event";
        demo.city = "Demo city";
        demo.country = "";
        demo.venue = "Your local test stage";
        demo.state = "UNKNOWN";
        demo.lastDefinite = "UNKNOWN";
        demo.detail = "Ready to simulate. This is not a real event or inventory result.";
        demo.demo = true;
        return demo;
    }

    private void startDemo() {
        saveSearchDraft();
        try { watches.save(demoWatch()); }
        catch (Exception error) { toast("Could not save the demo. Your watchlist may be full."); return; }
        watchFilter = "all";
        tab = 1;
        render();
    }

    private void simulateDemo(String state) {
        Models.Watch demo = demoWatch();
        if (watches.get(DEMO_ID) == null) watches.save(demo);
        String detail = "BLOCKED".equals(state)
            ? "Simulated blocked page. Availability is unknown; the last definite state is preserved."
            : "AVAILABLE".equals(state) ? "Two synthetic ticket offers found. Prices and quantities are demo data."
            : "Simulated no offers. Next, choose available to test one alert.";
        boolean transition = watches.observe(DEMO_ID, state, detail, "AVAILABLE".equals(state) ? demoSnapshot() : null);
        render();
        if (transition) notifyWithPermission(watches.get(DEMO_ID));
        else toast("Demo: " + stateLabel(state).toLowerCase(Locale.ROOT) + ". No new availability alert.");
    }

    /** Explicitly synthetic fixture for exercising ticket details without a network request. */
    public static TicketData.Snapshot demoSnapshot() {
        TicketData.Snapshot snapshot = new TicketData.Snapshot();
        snapshot.verified = true;
        snapshot.source = "demo";
        snapshot.observedAt = System.currentTimeMillis();
        snapshot.detail = "Synthetic offers for testing. Listing quantities are not total remaining inventory.";
        TicketData.Offer standard = new TicketData.Offer();
        standard.key = "demo-standard";
        standard.ticketType = "Standard admission";
        standard.section = "GA";
        standard.price = "435.85";
        standard.priceText = "435.85 PLN";
        standard.currency = "PLN";
        standard.priceScope = "per_ticket";
        standard.fees = "included";
        standard.quantity = 2;
        standard.quantityKind = "listing";
        standard.quantityText = "2";
        snapshot.offers.add(standard);
        TicketData.Offer platinum = new TicketData.Offer();
        platinum.key = "demo-platinum";
        platinum.ticketType = "Official Platinum";
        platinum.section = "TR-A9";
        platinum.row = "6";
        platinum.price = "910.80";
        platinum.priceText = "910.80 PLN";
        platinum.currency = "PLN";
        platinum.priceScope = "per_ticket";
        platinum.fees = "unknown";
        platinum.quantity = 4;
        platinum.quantityKind = "listing";
        platinum.quantityText = "4";
        snapshot.offers.add(platinum);
        return snapshot;
    }

    private void notifyWithPermission(Models.Watch watch) {
        if (watch == null) return;
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            pendingNotification = watch;
            requestPermissions(new String[] {Manifest.permission.POST_NOTIFICATIONS}, 44);
            return;
        }
        AlertController.notifyAvailability(this, watch);
        toast("Local test alert triggered. Check your notification panel.");
    }

    @Override public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(requestCode, permissions, results);
        if (requestCode != 44) return;
        Models.Watch pending = pendingNotification;
        pendingNotification = null;
        if (results.length > 0 && results[0] == PackageManager.PERMISSION_GRANTED && pending != null) {
            AlertController.notifyAvailability(this, pending);
            toast("Local test notification sent.");
        } else toast("Notifications are off. The demo result is still saved in your Watchlist.");
    }

    private void openUrl(String rawUrl) {
        String safe = DiscoveryClient.normalizePublicUrl(rawUrl);
        if (safe == null || safe.isEmpty()) { toast("This event has no supported official link."); return; }
        try { startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(safe))); }
        catch (Exception error) { toast("No browser is available to open this link."); }
    }

    private boolean isExternalSeller(String url) {
        if (blank(url)) return false;
        String host = Uri.parse(url).getHost();
        return host != null && !host.toLowerCase(Locale.ROOT).contains("ticketmaster.");
    }

    private void deliver(int request, Runnable task) {
        runOnUiThread(() -> { if (!destroyed && request == generation) task.run(); });
    }

    private void renderDiscoveryResult() {
        if (tab == 0) { saveSearchDraft(); render(); }
    }

    private String friendlyError(Exception error) {
        // Raw server exceptions may include credentials in a URL; never display them.
        String message = error.getMessage() == null ? "" : error.getMessage().toLowerCase(Locale.ROOT);
        if (message.contains("401") || message.contains("403") || message.contains("key") || message.contains("unauthor"))
            return "Discovery access was rejected. Check your API key in Settings and try again.";
        if (message.contains("429") || message.contains("rate")) return "Discovery is rate-limiting requests. Please try again later.";
        return "Could not load Discovery data. Check your connection and API key, then try again.";
    }

    private void saveSearchDraft() {
        if (artistInput != null) query = artistInput.getText().toString();
        if (countryInput != null) country = countryInput.getText().toString().trim().toUpperCase(Locale.ROOT);
        if (cityInput != null) city = cityInput.getText().toString().trim();
        if (preferences != null) preferences.edit().putString("explore_query", query).putString("explore_country", country).putString("explore_city", city).apply();
    }

    private boolean hasSecret(String name) { return !readSecret(name).isEmpty(); }
    private String readSecret(String name) {
        try { return nonempty(secrets.get(name), ""); }
        catch (Exception error) { return ""; }
    }
    private static boolean blank(String value) { return value == null || value.trim().isEmpty(); }
    private static String nonempty(String value, String fallback) { return blank(value) ? fallback : value; }
    private String stateLabel(String state) {
        if ("AVAILABLE".equals(state)) return "Available";
        if ("UNAVAILABLE".equals(state)) return "Unavailable";
        if ("BLOCKED".equals(state)) return "Blocked · unknown";
        return "Unknown";
    }
    private int stateColor(String state) {
        if ("AVAILABLE".equals(state)) return TEAL;
        if ("UNAVAILABLE".equals(state)) return MUTED;
        return AMBER;
    }
    private String eventMeta(String date, String city, String country) {
        String location = nonempty(city, "");
        if (!blank(country)) location += (location.isEmpty() ? "" : ", ") + country;
        return nonempty(date, "Date to be announced") + (location.isEmpty() ? "" : "  ·  " + location);
    }
    private void toast(String message) { android.widget.Toast.makeText(this, message, android.widget.Toast.LENGTH_LONG).show(); }
    private int dp(int value) { return Math.round(value * getResources().getDisplayMetrics().density); }
    private LinearLayout column() { LinearLayout view = new LinearLayout(this); view.setOrientation(LinearLayout.VERTICAL); return view; }
    private LinearLayout row() { LinearLayout view = new LinearLayout(this); view.setOrientation(LinearLayout.HORIZONTAL); return view; }
    private GradientDrawable shape(int color, int radius, int stroke) {
        GradientDrawable drawable = new GradientDrawable();
        drawable.setColor(color);
        drawable.setCornerRadius(dp(radius));
        if (stroke != 0) drawable.setStroke(dp(1), stroke);
        return drawable;
    }
    private LinearLayout card() {
        LinearLayout view = column();
        view.setPadding(dp(15), dp(15), dp(15), dp(15));
        view.setBackground(shape(CARD, 20, LINE));
        return view;
    }
    private TextView text(String value, int size, int color, boolean bold) {
        TextView view = new TextView(this);
        view.setText(value == null ? "" : value);
        view.setTextSize(size);
        view.setTextColor(color);
        view.setFontFeatureSettings("kern");
        view.setIncludeFontPadding(false);
        view.setLineSpacing(dp(3), 1f);
        if (bold) view.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        return view;
    }
    private TextView pill(String label, int color) {
        TextView view = text(label, 10, color, true);
        view.setLetterSpacing(.09f);
        view.setPadding(dp(10), dp(7), dp(10), dp(7));
        view.setBackground(shape(Color.argb(20, Color.red(color), Color.green(color), Color.blue(color)), 8, 0));
        view.setLayoutParams(new LinearLayout.LayoutParams(-2, -2));
        return view;
    }
    private void title(LinearLayout parent, String title) {
        TextView view = text(title, 26, TEXT, true);
        view.setLineSpacing(0, 1.05f);
        parent.addView(view);
        gap(parent, 8);
    }
    private void eyebrow(LinearLayout parent, String label) {
        TextView view = text(label, 10, TEAL, true);
        view.setLetterSpacing(.13f);
        parent.addView(view);
        gap(parent, 10);
    }
    private void paragraph(LinearLayout parent, String value) {
        TextView view = text(value, 13, MUTED, false);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
        params.topMargin = dp(8);
        params.bottomMargin = dp(4);
        parent.addView(view, params);
    }
    private void gap(LinearLayout parent, int height) { parent.addView(new View(this), new LinearLayout.LayoutParams(1, dp(height))); }
    private void section(LinearLayout parent, String title, String trailing) {
        LinearLayout line = row();
        line.setGravity(Gravity.CENTER_VERTICAL);
        line.addView(text(title, 19, TEXT, true), new LinearLayout.LayoutParams(0, -2, 1));
        line.addView(text(trailing, 11, MUTED, false));
        parent.addView(line);
        gap(parent, 12);
    }
    private EditText field(LinearLayout parent, String label, String hint, String value, boolean secret) {
        TextView caption = text(label, 11, MUTED, true);
        LinearLayout.LayoutParams captionParams = new LinearLayout.LayoutParams(-1, -2);
        captionParams.topMargin = dp(13);
        captionParams.bottomMargin = dp(7);
        parent.addView(caption, captionParams);
        EditText input = new EditText(this);
        input.setSingleLine(true);
        input.setTextColor(TEXT);
        input.setHintTextColor(Color.rgb(100, 125, 147));
        input.setTextSize(14);
        input.setPadding(dp(14), dp(13), dp(14), dp(13));
        input.setBackground(shape(FIELD, 10, LINE));
        input.setInputType(InputType.TYPE_CLASS_TEXT | (secret ? InputType.TYPE_TEXT_VARIATION_PASSWORD : InputType.TYPE_TEXT_FLAG_CAP_SENTENCES));
        if (secret) {
            input.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS);
            input.setImeOptions(android.view.inputmethod.EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING);
            input.setSaveEnabled(false);
        }
        input.setHint(hint);
        input.setContentDescription(label);
        input.setMinimumHeight(dp(48));
        input.setText(value);
        parent.addView(input, new LinearLayout.LayoutParams(-1, -2));
        return input;
    }
    private EditText dialogField(String hint, String value, boolean secret) {
        EditText input = new EditText(this);
        input.setSingleLine(true);
        input.setHint(hint);
        input.setText(value);
        input.setTextSize(14);
        input.setInputType(InputType.TYPE_CLASS_TEXT | (secret ? InputType.TYPE_TEXT_VARIATION_PASSWORD : 0));
        input.setPadding(dp(10), dp(14), dp(10), dp(14));
        return input;
    }
    private View dialogWrap(View child) {
        LinearLayout wrapper = column();
        wrapper.setPadding(dp(20), dp(8), dp(20), dp(8));
        wrapper.addView(child);
        return wrapper;
    }
    private Button button(String label, boolean primary, Runnable click) {
        Button button = new Button(this);
        button.setText(label);
        button.setTextSize(13);
        button.setAllCaps(false);
        button.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        button.setTextColor(primary ? BG : TEAL);
        button.setBackground(shape(primary ? TEAL : FIELD, 11, primary ? 0 : LINE));
        button.setMinHeight(dp(48));
        button.setMinimumHeight(dp(48));
        button.setPadding(dp(12), dp(11), dp(12), dp(11));
        if (click != null) button.setOnClickListener(view -> click.run());
        return button;
    }
    private Button action(LinearLayout parent, String label, boolean primary, Runnable click) {
        Button button = button(label, primary, click);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
        params.topMargin = dp(12);
        parent.addView(button, params);
        return button;
    }
    private void addRowButton(LinearLayout row, String label, boolean primary, Runnable click) {
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(0, -2, 1);
        params.topMargin = dp(15);
        if (row.getChildCount() == 0) params.rightMargin = dp(8);
        row.addView(button(label, primary, click), params);
    }
    private TextView chip(String label, boolean selected, Runnable click) {
        TextView view = text(label, 12, selected ? BG : MUTED, true);
        view.setGravity(Gravity.CENTER);
        view.setMinHeight(dp(48));
        view.setPadding(dp(4), dp(8), dp(4), dp(8));
        view.setFocusable(true);
        styleChip(view, selected);
        view.setOnClickListener(ignored -> click.run());
        return view;
    }
    private void styleChip(TextView view, boolean selected) {
        view.setSelected(selected);
        view.setTextColor(selected ? BG : MUTED);
        view.setBackground(shape(selected ? TEAL : FIELD, 11, selected ? 0 : LINE));
    }
    private void stat(LinearLayout row, String value, String label) {
        LinearLayout box = card();
        box.addView(text(value, 25, TEAL, true));
        gap(box, 6);
        TextView caption = text(label, 9, MUTED, true);
        caption.setLetterSpacing(.07f);
        box.addView(caption);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(0, -2, 1);
        if (row.getChildCount() == 0) params.rightMargin = dp(12);
        row.addView(box, params);
    }
    private void footer(LinearLayout parent) {
        gap(parent, 22);
        TextView footer = text("DISCOVERY ≠ LIVE INVENTORY\nManual checks · No ticket purchases", 10, MUTED, false);
        footer.setGravity(Gravity.CENTER);
        footer.setLetterSpacing(.03f);
        parent.addView(footer);
    }
}
