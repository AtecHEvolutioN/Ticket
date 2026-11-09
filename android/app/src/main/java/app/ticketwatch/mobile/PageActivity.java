package app.ticketwatch.mobile;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.StateListDrawable;
import android.net.Uri;
import android.net.http.SslError;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.text.Editable;
import android.text.InputFilter;
import android.text.TextWatcher;
import android.view.View;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.Window;
import android.view.WindowManager;
import android.webkit.GeolocationPermissions;
import android.webkit.PermissionRequest;
import android.webkit.SslErrorHandler;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.ImageButton;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;
import android.widget.Spinner;
import android.widget.ArrayAdapter;
import android.widget.AdapterView;

import org.json.JSONObject;
import org.json.JSONTokener;

import java.io.ByteArrayInputStream;
import java.text.DateFormat;
import java.util.Collections;
import java.util.Date;

/** A foreground, user-driven page test. This is deliberately not a background monitor. */
public final class PageActivity extends Activity {
    private static final long RENDER_WAIT_MS = 1500;
    private static final long TIMEOUT_MS = 30000;
    private static final int BG = Color.rgb(9, 17, 29);
    private static final int CARD = Color.rgb(18, 31, 47);
    private static final int TEAL = Color.rgb(89, 229, 202);
    private static final int TEXT = Color.rgb(237, 244, 250);
    private static final int MUTED = Color.rgb(153, 174, 192);
    private static final int BORDER = Color.rgb(38, 57, 76);
    private static final int AMBER = Color.rgb(255, 203, 118);
    private final Handler handler = new Handler(Looper.getMainLooper());
    private WatchStore store;
    private Models.Watch watch;
    private String watchId;
    private WebView web;
    private TextView status;
    private TextView stateLabel;
    private TextView checkedLabel;
    private Button check;
    private EditText availableInput;
    private EditText unavailableInput;
    private CheckBox verifiedInput;
    private LinearLayout ticketDetails;
    private EditText offerRowsInput, offerAvailableInput, offerTypeInput, offerSectionInput, offerPriceInput, offerQuantityInput, offerCurrencyInput;
    private Spinner offerDecimalInput, offerQuantityKindInput, offerPriceScopeInput, offerFeesInput;
    private CheckBox offersVerifiedInput;
    private boolean active;
    private boolean destroyed;
    private boolean initialLoad;
    private boolean supported;
    private boolean failed;
    private boolean settled;
    private boolean prepared;
    private boolean loading;
    private boolean interrupted;
    private boolean rendererUnavailable;
    private boolean ticketDisplayUnavailable;
    private String failureState = "";
    private int generation;
    private long deadline;
    private String currentUrl = "";
    private Runnable timeout;
    private Runnable inspection;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        requestWindowFeature(Window.FEATURE_NO_TITLE);
        store = new WatchStore(this);
        watchId = getIntent().getStringExtra("watch_id");
        watch = store.get(watchId == null ? "" : watchId);
        if (watch == null) {
            Toast.makeText(this, "This watch no longer exists.", Toast.LENGTH_LONG).show();
            finish();
            return;
        }
        supported = !watch.demo && PageSignals.isTicketmasterUrl(watch.url);
        buildUi();
        configureWebView();
        renderStatus();
        if (watch.demo) {
            showStatus("DEMO PREVIEW", "A synthetic event for exploring the app. Try its demo check in your watch list; this screen does not load a live ticket page.");
        } else if (!supported) {
            showStatus("UNKNOWN", "In-app checks are not supported for this provider. Open the public event in your browser. This does not mean tickets are sold out.");
        }
    }

    private void buildUi() {
        getWindow().setStatusBarColor(BG);
        getWindow().setNavigationBarColor(BG);
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
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
        back.setBackground(buttonBackground(false));
        back.setContentDescription("Back to watchlist");
        back.setPadding(dp(13), dp(13), dp(13), dp(13));
        back.setOnClickListener(view -> finish());
        heading.addView(back, new LinearLayout.LayoutParams(dp(48), dp(48)));
        TextView headingTitle = text("Page inspector", 18);
        headingTitle.setTypeface(null, Typeface.BOLD);
        headingTitle.setPadding(dp(12), 0, 0, 0);
        heading.addView(headingTitle, new LinearLayout.LayoutParams(0, -2, 1));
        root.addView(heading, new LinearLayout.LayoutParams(-1, -2));

        // The complete body scrolls, including the expanded editor. The embedded page keeps
        // a useful viewport even when the keyboard or accessibility font size needs room.
        ScrollView bodyScroll = new ScrollView(this);
        bodyScroll.setFillViewport(true);
        bodyScroll.setClipToPadding(false);
        bodyScroll.setVerticalScrollBarEnabled(false);
        LinearLayout body = column();
        body.setPadding(dp(20), dp(12), dp(20), dp(28));
        bodyScroll.addView(body, new ScrollView.LayoutParams(-1, -2));
        root.addView(bodyScroll, new LinearLayout.LayoutParams(-1, 0, 1));

        TextView eyebrow = text(watch.demo ? "DEMO WATCH" : "EVENT PAGE", 11);
        eyebrow.setTextColor(TEAL);
        eyebrow.setTypeface(null, Typeface.BOLD);
        eyebrow.setLetterSpacing(.13f);
        body.addView(eyebrow);
        TextView title = text(watch.name, 24);
        title.setTypeface(null, Typeface.BOLD);
        add(body, title, 7);
        String location = watch.venue;
        if (!watch.city.isEmpty()) location += (location.isEmpty() ? "" : " · ") + watch.city;
        if (!watch.date.isEmpty()) location += (location.isEmpty() ? "" : "\n") + watch.date;
        if (!location.isEmpty()) {
            TextView subtitle = text(location, 13);
            subtitle.setTextColor(MUTED);
            subtitle.setLineSpacing(dp(3), 1);
            add(body, subtitle, 6);
        }

        ticketDetails = column();
        add(body, ticketDetails, 16);
        renderTicketDetails();

        LinearLayout statusCard = column();
        statusCard.setPadding(dp(16), dp(15), dp(16), dp(15));
        statusCard.setBackground(surface(CARD, BORDER, 16));
        stateLabel = text("", 12);
        stateLabel.setLetterSpacing(.05f);
        stateLabel.setTypeface(null, Typeface.BOLD);
        statusCard.addView(stateLabel);
        status = text("", 14);
        status.setLineSpacing(dp(3), 1);
        status.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        add(statusCard, status, 9);
        checkedLabel = text("", 11);
        checkedLabel.setTextColor(MUTED);
        add(statusCard, checkedLabel, 12);
        add(body, statusCard, 20);

        LinearLayout actions = new LinearLayout(this);
        check = button("Check page", true);
        check.setId(R.id.ui_check_page);
        check.setEnabled(supported);
        check.setOnClickListener(view -> checkCurrentPage());
        actions.addView(check, new LinearLayout.LayoutParams(0, -2, 1));
        Button browser = button("Open browser", false);
        browser.setId(R.id.ui_open_browser);
        browser.setEnabled(!DiscoveryClient.normalizePublicUrl(watch.url).isEmpty());
        browser.setOnClickListener(view -> openExternal(watch.url));
        LinearLayout.LayoutParams browserParams = new LinearLayout.LayoutParams(0, -2, 1);
        browserParams.leftMargin = dp(10);
        actions.addView(browser, browserParams);
        add(body, actions, 14);
        TextView warning = text("Checks recognize supported ticket layouts automatically for any selected event. Queues or human checks may need your browser.", 12);
        warning.setTextColor(MUTED);
        warning.setLineSpacing(dp(2), 1);
        add(body, warning, 10);

        Button advanced = button("Advanced selectors  ›", false);
        advanced.setId(R.id.ui_advanced);
        advanced.setGravity(Gravity.CENTER_VERTICAL | Gravity.START);
        advanced.setTextColor(new ColorStateList(new int[][] { new int[] { -android.R.attr.state_enabled }, new int[0] },
                new int[] { MUTED, TEXT }));
        advanced.setBackgroundColor(Color.TRANSPARENT);
        advanced.setPadding(0, 0, 0, 0);
        advanced.setEnabled(supported);
        add(body, advanced, 8);
        LinearLayout editor = column();
        editor.setVisibility(View.GONE);
        editor.setPadding(dp(14), dp(15), dp(14), dp(15));
        editor.setBackground(surface(CARD, BORDER, 14));
        TextView explanation = text("Use signals you have verified for this event. A readable page alone does not confirm ticket availability. Editing either signal clears verification.", 12);
        explanation.setTextColor(MUTED);
        explanation.setLineSpacing(dp(3), 1);
        editor.addView(explanation);
        TextView availableLabel = text("Available signal · CSS selector", 12);
        add(editor, availableLabel, 16);
        availableInput = selectorInput(watch.availableSelector);
        availableInput.setContentDescription("Available signal CSS selector");
        availableLabel.setLabelFor(availableInput.getId());
        add(editor, availableInput, 6);
        TextView unavailableLabel = text("Unavailable signal · CSS selector", 12);
        add(editor, unavailableLabel, 14);
        unavailableInput = selectorInput(watch.unavailableSelector);
        unavailableInput.setContentDescription("Unavailable signal CSS selector");
        unavailableLabel.setLabelFor(unavailableInput.getId());
        add(editor, unavailableInput, 6);
        verifiedInput = new CheckBox(this);
        verifiedInput.setText("I verified these signals for this event");
        verifiedInput.setTextColor(TEXT);
        verifiedInput.setTextSize(13);
        verifiedInput.setButtonTintList(ColorStateList.valueOf(TEAL));
        verifiedInput.setMinHeight(dp(48));
        verifiedInput.setChecked(watch.selectorsVerified);
        add(editor, verifiedInput, 8);
        TextWatcher edits = new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int start, int count, int after) { }
            @Override public void onTextChanged(CharSequence s, int start, int before, int count) { verifiedInput.setChecked(false); }
            @Override public void afterTextChanged(Editable value) { }
        };
        availableInput.addTextChangedListener(edits);
        unavailableInput.addTextChangedListener(edits);
        buildOfferEditor(editor);
        Button save = button("Save selectors", false);
        save.setId(R.id.ui_save_selectors);
        save.setOnClickListener(view -> saveSelectors());
        add(editor, save, 6);
        body.addView(editor, new LinearLayout.LayoutParams(-1, -2));
        advanced.setOnClickListener(view -> {
            boolean show = editor.getVisibility() != View.VISIBLE;
            editor.setVisibility(show ? View.VISIBLE : View.GONE);
            advanced.setText(show ? "Advanced selectors  ⌄" : "Advanced selectors  ›");
            if (!show) {
                android.view.inputmethod.InputMethodManager keyboard = (android.view.inputmethod.InputMethodManager)
                        getSystemService(INPUT_METHOD_SERVICE);
                if (keyboard != null) keyboard.hideSoftInputFromWindow(editor.getWindowToken(), 0);
                editor.clearFocus();
            }
        });

        LinearLayout pageCard = column();
        pageCard.setBackground(surface(CARD, BORDER, 14));
        pageCard.setClipToOutline(true);
        TextView pageHeading = text(watch.demo ? "EVENT PAGE PREVIEW" : "PUBLIC EVENT PAGE", 10);
        pageHeading.setTypeface(null, Typeface.BOLD);
        pageHeading.setLetterSpacing(.08f);
        pageHeading.setTextColor(MUTED);
        pageHeading.setPadding(dp(14), dp(13), dp(14), dp(13));
        pageCard.addView(pageHeading);
        web = new WebView(this);
        web.setBackgroundColor(CARD);
        web.setOnTouchListener((view, event) -> {
            if (event.getActionMasked() == MotionEvent.ACTION_DOWN) bodyScroll.requestDisallowInterceptTouchEvent(true);
            if (event.getActionMasked() == MotionEvent.ACTION_UP || event.getActionMasked() == MotionEvent.ACTION_CANCEL)
                bodyScroll.requestDisallowInterceptTouchEvent(false);
            return false;
        });
        pageCard.addView(web, new LinearLayout.LayoutParams(-1, dp(360)));
        if (!supported) {
            web.setVisibility(View.GONE);
            LinearLayout placeholder = column();
            placeholder.setGravity(Gravity.CENTER);
            placeholder.setPadding(dp(24), dp(26), dp(24), dp(32));
            ImageView icon = new ImageView(this);
            icon.setImageResource(R.drawable.ic_explore);
            icon.setImageTintList(ColorStateList.valueOf(TEAL));
            placeholder.addView(icon, new LinearLayout.LayoutParams(dp(34), dp(34)));
            TextView placeholderTitle = text(watch.demo ? "Your event page, in one place" : "Continue in your browser", 16);
            placeholderTitle.setGravity(Gravity.CENTER);
            placeholderTitle.setTypeface(null, Typeface.BOLD);
            add(placeholder, placeholderTitle, 16);
            TextView placeholderNote = text(watch.demo ? "Real Ticketmaster watches load their public page here. This demo stays offline."
                    : "This provider needs your browser. Use Open browser to view the event.", 13);
            placeholderNote.setTextColor(MUTED);
            placeholderNote.setGravity(Gravity.CENTER);
            placeholderNote.setLineSpacing(dp(3), 1);
            add(placeholder, placeholderNote, 8);
            pageCard.addView(placeholder);
        }
        add(body, pageCard, 8);
        setContentView(root);
    }

    @SuppressLint("SetJavaScriptEnabled") private void configureWebView() {
        WebSettings settings = web.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setAllowFileAccessFromFileURLs(false);
        settings.setAllowUniversalAccessFromFileURLs(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setGeolocationEnabled(false);
        settings.setJavaScriptCanOpenWindowsAutomatically(false);
        settings.setSupportMultipleWindows(false);
        settings.setSafeBrowsingEnabled(true);
        settings.setMediaPlaybackRequiresUserGesture(true);
        web.removeJavascriptInterface("searchBoxJavaBridge_");
        web.removeJavascriptInterface("accessibility");
        web.removeJavascriptInterface("accessibilityTraversal");
        // No JavaScript interface, Discovery key, injected header, or alternate source is exposed.
        web.setWebChromeClient(new WebChromeClient() {
            @Override public void onGeolocationPermissionsShowPrompt(String origin, GeolocationPermissions.Callback callback) {
                callback.invoke(origin, false, false);
            }
            @Override public void onPermissionRequest(PermissionRequest request) { request.deny(); }
        });
        web.setWebViewClient(new WebViewClient() {
            @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                String url = request.getUrl().toString();
                if (!PageSignals.isHttps(url)) {
                    if (request.isForMainFrame()) navigationBlocked("Only ordinary HTTPS pages can open here.");
                    return true;
                }
                if (request.isForMainFrame() && !PageSignals.isTicketmasterUrl(url)) {
                    if (request.hasGesture()) openExternal(url);
                    navigationBlocked("This destination needs your browser. Automatic navigation outside Ticketmaster was stopped.");
                    return true;
                }
                if (request.isForMainFrame() && request.hasGesture()) {
                    // A new user navigation is a new attempt; redirects share its original deadline.
                    deadline = 0;
                    failed = false;
                    settled = false;
                }
                return false;
            }

            @Override public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
                String url = request.getUrl().toString();
                if (!PageSignals.isHttps(url) || (request.isForMainFrame() && !PageSignals.isTicketmasterUrl(url))) {
                    return new WebResourceResponse("text/plain", "UTF-8", 403, "Blocked by app",
                            Collections.emptyMap(), new ByteArrayInputStream(new byte[0]));
                }
                return null;
            }

            @Override public void onPageStarted(WebView view, String url, android.graphics.Bitmap icon) {
                if (!active || destroyed) return;
                if (failed) { view.stopLoading(); return; }
                startCycle(url, true);
                if (!PageSignals.isTicketmasterUrl(url)) {
                    view.stopLoading();
                    fail("UNKNOWN", "This destination cannot be checked in the app. Open the public event in your browser.");
                }
            }

            @Override public void onPageFinished(WebView view, String url) {
                if (!valid(generation) || failed || settled) return;
                if (!sameUrl(url, web.getUrl()) || !sameUrl(url, currentUrl)) return;
                loading = false;
                scheduleInspection(generation);
            }

            @Override public void onReceivedHttpError(WebView view, WebResourceRequest request, WebResourceResponse response) {
                if (!request.isForMainFrame() || !matchesCurrentRequest(request.getUrl().toString())) return;
                int code = response.getStatusCode();
                boolean blocked = code == 401 || code == 403 || code == 429;
                fail(blocked ? "BLOCKED" : "UNKNOWN", "The page returned HTTP " + code
                        + (blocked ? ". Access is restricted; use the browser if a human check is required." : ". Availability could not be checked."));
            }

            @Override public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
                if (request.isForMainFrame() && matchesCurrentRequest(request.getUrl().toString())) {
                    fail("UNKNOWN", "The page could not load (browser error " + error.getErrorCode() + "). Try again manually or open your browser.");
                }
            }

            @Override public void onReceivedSslError(WebView view, SslErrorHandler sslHandler, SslError error) {
                sslHandler.cancel();
                if (matchesCurrentRequest(error.getUrl())) fail("BLOCKED", "The page's secure connection could not be verified. The app canceled it.");
            }

            @Override public boolean onRenderProcessGone(WebView view, android.webkit.RenderProcessGoneDetail detail) {
                rendererUnavailable = true;
                if (active && !destroyed) fail("UNKNOWN", "The browser stopped. Return to your watches and reopen this page.");
                check.setEnabled(false);
                return true;
            }
        });
    }

    private void buildOfferEditor(LinearLayout editor) {
        TicketData.Profile profile = watch.offerProfile;
        TextView title = text("Ticket offer details", 17);
        title.setTypeface(null, Typeface.BOLD);
        add(editor, title, 22);
        TextView note = text("Configure fields within one visible, purchasable offer row. These rules vary by event and country. A quantity dropdown is an order choice, not remaining stock. Both event signals and these offer rules must be verified before details are accepted.", 12);
        note.setTextColor(MUTED);
        note.setLineSpacing(dp(3), 1);
        add(editor, note, 8);
        offersVerifiedInput = new CheckBox(this);
        offersVerifiedInput.setText("I verified these offer fields and their meanings for this event");
        offersVerifiedInput.setTextSize(13);
        offersVerifiedInput.setTextColor(TEXT);
        offersVerifiedInput.setButtonTintList(ColorStateList.valueOf(TEAL));
        offersVerifiedInput.setMinHeight(dp(48));
        offersVerifiedInput.setChecked(profile.verified);
        offerRowsInput = offerField(editor, "Offer row · CSS selector", profile.rowSelector, "offer_rows");
        offerAvailableInput = offerField(editor, "Purchasable signal within row · CSS selector", profile.availableSelector, "offer_available");
        offerTypeInput = offerField(editor, "Ticket type within row · CSS selector", profile.typeSelector, "offer_type");
        offerSectionInput = offerField(editor, "Section within row · CSS selector", profile.sectionSelector, "offer_section");
        offerPriceInput = offerField(editor, "Price within row · CSS selector", profile.priceSelector, "offer_price");
        offerQuantityInput = offerField(editor, "Quantity within row · CSS selector", profile.quantitySelector, "offer_quantity");
        offerCurrencyInput = offerField(editor, "Currency · e.g. PLN, EUR (blank if unknown)", profile.currency, "offer_currency");
        offerCurrencyInput.setFilters(new InputFilter[]{new InputFilter.LengthFilter(3), new InputFilter.AllCaps()});
        offerDecimalInput = offerChoice(editor, "Price decimal separator", new String[]{"Decimal point · 910.80", "Decimal comma · 910,80"},
                ",".equals(profile.decimalSeparator) ? 1 : 0);
        offerQuantityKindInput = offerChoice(editor, "What the displayed quantity means", new String[]{"Unknown", "Tickets in this listing", "Selectable order quantity", "Reported remaining in this offer"},
                indexOf(profile.quantityKind, new String[]{"unknown", "listing", "selectable", "remaining"}));
        offerPriceScopeInput = offerChoice(editor, "What the displayed price covers", new String[]{"Unknown", "Per ticket", "Offer total"},
                indexOf(profile.priceScope, new String[]{"unknown", "per_ticket", "total"}));
        offerFeesInput = offerChoice(editor, "Fees in the displayed price", new String[]{"Unknown", "Included", "Excluded"},
                indexOf(profile.fees, new String[]{"unknown", "included", "excluded"}));
        add(editor, offersVerifiedInput, 12);
    }

    private EditText offerField(LinearLayout editor, String label, String value, String tag) {
        TextView caption = text(label, 12);
        caption.setTextColor(MUTED);
        add(editor, caption, 14);
        EditText input = selectorInput(value);
        input.setTag(tag);
        input.setContentDescription(label);
        caption.setLabelFor(input.getId());
        add(editor, input, 6);
        input.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence value, int start, int count, int after) { }
            @Override public void onTextChanged(CharSequence value, int start, int before, int count) { offersVerifiedInput.setChecked(false); }
            @Override public void afterTextChanged(Editable value) { }
        });
        return input;
    }

    private Spinner offerChoice(LinearLayout editor, String label, String[] choices, int initial) {
        TextView caption = text(label, 12);
        caption.setTextColor(MUTED);
        add(editor, caption, 14);
        Spinner spinner = new Spinner(this);
        spinner.setContentDescription(label);
        spinner.setMinimumHeight(dp(48));
        ArrayAdapter<String> adapter = new ArrayAdapter<>(this, android.R.layout.simple_spinner_item, choices);
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        spinner.setAdapter(adapter);
        spinner.setSelection(initial);
        final int[] lastSelection = {initial};
        spinner.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            @Override public void onItemSelected(AdapterView<?> parent, View selected, int position, long id) {
                if (position != lastSelection[0]) offersVerifiedInput.setChecked(false);
                lastSelection[0] = position;
            }
            @Override public void onNothingSelected(AdapterView<?> parent) { }
        });
        add(editor, spinner, 4);
        return spinner;
    }

    private static int indexOf(String value, String[] choices) {
        for (int i = 0; i < choices.length; i++) if (choices[i].equals(value)) return i;
        return 0;
    }

    private void saveSelectors() {
        String available = availableInput.getText().toString().trim();
        String unavailable = unavailableInput.getText().toString().trim();
        boolean verified = verifiedInput.isChecked();
        if (verified && (available.isEmpty() || unavailable.isEmpty())) {
            Toast.makeText(this, "Enter both signals before verifying, or leave verification unchecked.", Toast.LENGTH_LONG).show();
            return;
        }
        TicketData.Profile profile = new TicketData.Profile();
        profile.rowSelector = offerRowsInput.getText().toString().trim();
        profile.availableSelector = offerAvailableInput.getText().toString().trim();
        profile.typeSelector = offerTypeInput.getText().toString().trim();
        profile.sectionSelector = offerSectionInput.getText().toString().trim();
        profile.priceSelector = offerPriceInput.getText().toString().trim();
        profile.quantitySelector = offerQuantityInput.getText().toString().trim();
        profile.currency = offerCurrencyInput.getText().toString().trim().toUpperCase(java.util.Locale.ROOT);
        profile.decimalSeparator = offerDecimalInput.getSelectedItemPosition() == 1 ? "," : ".";
        profile.quantityKind = new String[]{"unknown", "listing", "selectable", "remaining"}[offerQuantityKindInput.getSelectedItemPosition()];
        profile.priceScope = new String[]{"unknown", "per_ticket", "total"}[offerPriceScopeInput.getSelectedItemPosition()];
        profile.fees = new String[]{"unknown", "included", "excluded"}[offerFeesInput.getSelectedItemPosition()];
        profile.verified = offersVerifiedInput.isChecked();
        if (profile.verified && (profile.rowSelector.isEmpty() || profile.availableSelector.isEmpty())) {
            Toast.makeText(this, "Verified offers need a row selector and a purchasable signal inside each row.", Toast.LENGTH_LONG).show();
            return;
        }
        if (!profile.currency.isEmpty()) {
            try { java.util.Currency.getInstance(profile.currency); }
            catch (IllegalArgumentException error) {
                Toast.makeText(this, "Use a three-letter currency code such as PLN or EUR, or leave it unknown.", Toast.LENGTH_LONG).show();
                return;
            }
        }
        Models.Watch latest = store.get(watchId);
        if (latest == null) { finish(); return; }
        latest.availableSelector = available;
        latest.unavailableSelector = unavailable;
        latest.selectorsVerified = verified;
        latest.offerProfile = profile;
        try {
            store.save(latest);
            watch = latest;
            cancelCycle();
            settled = true;
            record("UNKNOWN", verified ? "Page and offer rules saved. Check the page to inspect visible ticket details." : "Selectors are not verified. Readable pages do not establish availability.");
            Toast.makeText(this, "Selectors saved", Toast.LENGTH_SHORT).show();
        } catch (RuntimeException error) { Toast.makeText(this, "Selectors could not be saved. Try again.", Toast.LENGTH_LONG).show(); }
    }

    private void checkCurrentPage() {
        if (!active || !supported || destroyed) return;
        Models.Watch latest = store.get(watchId);
        if (latest == null) { finish(); return; }
        watch = latest;
        deadline = 0;
        String url = web.getUrl();
        if (url == null || url.isEmpty() || failed) {
            web.stopLoading();
            startCycle(watch.url, true);
            web.loadUrl(watch.url);
        } else {
            // Reinspect the ordinary page already on screen; there is no automatic refresh loop.
            startCycle(url, false);
            scheduleInspection(generation);
        }
    }

    private void startCycle(String url, boolean isLoading) {
        if (deadline == 0 || settled) deadline = SystemClock.elapsedRealtime() + TIMEOUT_MS;
        cancelCycle();
        failed = false;
        failureState = "";
        settled = false;
        prepared = false;
        loading = isLoading;
        currentUrl = url == null ? "" : url;
        final int token = generation;
        showStatus("CHECKING", loading ? "Loading the public event page…" : "Waiting for visible page signals…");
        timeout = () -> {
            if (valid(token) && !settled && !failed) {
                web.stopLoading();
                fail("UNKNOWN", "The page check timed out after 30 seconds. No availability conclusion was made.");
            }
        };
        handler.postDelayed(timeout, Math.max(1, deadline - SystemClock.elapsedRealtime()));
    }

    private void scheduleInspection(int token) {
        if (inspection != null) handler.removeCallbacks(inspection);
        inspection = () -> inspect(token);
        handler.postDelayed(inspection, RENDER_WAIT_MS);
    }

    private void inspect(int token) {
        if (!valid(token) || failed || settled) return;
        Models.Watch latest = store.get(watchId);
        if (latest == null) { finish(); return; }
        watch = latest;
        final String expectedUrl = watch.url;
        final String available = watch.availableSelector;
        final String unavailable = watch.unavailableSelector;
        final boolean verificationSetting = watch.selectorsVerified;
        final TicketData.Profile offerProfile = TicketData.profileFromJson(TicketData.profileToJson(watch.offerProfile));
        final boolean verified = verificationSetting && !available.isEmpty() && !unavailable.isEmpty();
        final String atEvaluation = web.getUrl();
        if (!verified && !prepared && PageSignals.sameEvent(expectedUrl, atEvaluation)) {
            prepared = true;
            web.evaluateJavascript(GenericTicketParser.prepareScript(), ignored -> {
                if (valid(token) && !failed && !settled && sameUrl(atEvaluation, web.getUrl())) scheduleInspection(token);
            });
            return;
        }
        String script = "(function(){try{var r={url:location.href,text:(document.body?document.body.innerText:'').slice(0,18000),"
                + "ready:document.readyState,valid:true,available:false,unavailable:false};"
                + "function visible(e){if(!e||!e.isConnected)return false;for(var p=e;p&&p.nodeType===1;p=p.parentElement){"
                + "var s=getComputedStyle(p);if(p.hidden||p.getAttribute('aria-hidden')==='true'||s.display==='none'||s.visibility==='hidden'||s.visibility==='collapse'||s.contentVisibility==='hidden'||Number(s.opacity)===0)return false;}"
                + "var rects=e.getClientRects();for(var i=0;i<rects.length;i++){if(rects[i].width>0&&rects[i].height>0)return true;}return false;}"
                + "function match(selector){var list=document.querySelectorAll(selector);for(var i=0;i<Math.min(list.length,400);i++){if(visible(list[i]))return true;}return false;}"
                + "if(" + verified + "){try{r.available=match(" + JSONObject.quote(available) + ");r.unavailable=match(" + JSONObject.quote(unavailable) + ");}catch(e){r.valid=false;}}"
                + "r.offers=" + (verified ? TicketPageParser.extractionScript(offerProfile) : GenericTicketParser.extractionScript()) + ";"
                + "return JSON.stringify(r);}catch(e){return JSON.stringify({valid:false,text:'',url:location.href});}})();";
        web.evaluateJavascript(script, result -> {
            if (!valid(token) || failed || settled || !sameUrl(atEvaluation, web.getUrl())) return;
            Models.Watch current = store.get(watchId);
            if (current == null) { finish(); return; }
            // A settings edit or replacement watch makes this callback stale.
            if (!expectedUrl.equals(current.url) || !available.equals(current.availableSelector)
                    || !unavailable.equals(current.unavailableSelector) || verificationSetting != current.selectorsVerified
                    || !TicketData.sameProfile(offerProfile, current.offerProfile)) return;
            try {
                Object decoded = new JSONTokener(result == null ? "null" : result).nextValue();
                if (!(decoded instanceof String)) throw new IllegalStateException();
                JSONObject signals = new JSONObject((String) decoded);
                String finalUrl = signals.optString("url", "");
                if (!sameUrl(finalUrl, web.getUrl())) {
                    complete("UNKNOWN", "The page changed during inspection. Check it again manually.");
                    return;
                }
                boolean identity = PageSignals.sameEvent(expectedUrl, finalUrl);
                String text = signals.optString("text", "");
                boolean validSelectors = signals.optBoolean("valid", false);
                boolean availableMatch = signals.optBoolean("available", false);
                boolean unavailableMatch = signals.optBoolean("unavailable", false);
                String state = PageSignals.classify(identity, verified, validSelectors, availableMatch, unavailableMatch, text);
                String detail;
                if ("BLOCKED".equals(state)) detail = "A queue, challenge, or access restriction was detected. Continue manually in your browser; the app does not bypass it.";
                else if (!identity) detail = "This page could not be matched to the watched host and event ID. No availability conclusion was made.";
                else if (!verified) detail = "This ticket layout is unsupported or not fully loaded. Availability remains unknown.";
                else if (!validSelectors) detail = "A selector is invalid or the page could not be inspected. Check your Advanced selectors.";
                else if (availableMatch && unavailableMatch) detail = "Both verified signals are visible. They conflict, so availability is unknown.";
                else if (!availableMatch && !unavailableMatch) detail = "Neither verified signal is visible. Missing signals do not mean sold out.";
                else if ("AVAILABLE".equals(state)) detail = "Your verified available signal is visible. Open the event to confirm tickets before acting.";
                else detail = "Your verified unavailable signal is visible. This is a page observation, not a guarantee about all inventory.";
                TicketData.Snapshot tickets = verified
                        ? TicketPageParser.parse(signals.optJSONObject("offers"), offerProfile, identity, state, System.currentTimeMillis())
                        : GenericTicketParser.parse(signals.optJSONObject("offers"), identity, state, System.currentTimeMillis());
                if (!verified && tickets.verified) {
                    state = tickets.offers.isEmpty() ? "UNKNOWN" : "AVAILABLE";
                    detail = tickets.detail;
                }
                complete(state, detail, tickets);
            } catch (Exception error) { complete("UNKNOWN", "The page response could not be read. No availability conclusion was made."); }
        });
    }

    private void navigationBlocked(String detail) {
        if (active && !destroyed) fail("UNKNOWN", detail);
    }

    private boolean matchesCurrentRequest(String url) {
        return active && !destroyed && (sameUrl(url, currentUrl) || sameUrl(url, web.getUrl()));
    }

    private void fail(String state, String detail) {
        if (!active || destroyed) return;
        if (failed && "BLOCKED".equals(failureState) && !"BLOCKED".equals(state)) return;
        failed = true;
        failureState = state;
        settled = true;
        removeScheduled();
        record(state, detail);
    }

    private void complete(String state, String detail) {
        complete(state, detail, null);
    }

    private void complete(String state, String detail, TicketData.Snapshot tickets) {
        if (!active || destroyed || failed || settled) return;
        settled = true;
        removeScheduled();
        record(state, detail, tickets);
    }

    private void record(String state, String detail) {
        record(state, detail, null);
    }

    private void record(String state, String detail, TicketData.Snapshot tickets) {
        if (!active || destroyed || isFinishing()) return;
        Models.Watch latest = store.get(watchId);
        if (latest == null) { finish(); return; }
        try {
            boolean transition = store.observe(watchId, state, detail, tickets);
            watch = store.get(watchId);
            if (watch == null) { finish(); return; }
            ticketDisplayUnavailable = false;
            renderStatus();
            if (transition) AlertController.notifyAvailability(this, watch);
        } catch (RuntimeException error) {
            ticketDisplayUnavailable = true;
            showStatus("UNKNOWN", "The observation could not be saved. Try again manually.");
            renderTicketDetails();
        }
    }

    private void renderStatus() {
        if (ticketDisplayUnavailable) showStatus("UNKNOWN", "The latest observation could not be saved. Check the page again before relying on ticket details.");
        else showStatus(watch.state, watch.detail);
        renderTicketDetails();
    }

    private void renderTicketDetails() {
        if (ticketDetails == null || watch == null) return;
        ticketDetails.removeAllViews();
        TicketViews.addSnapshot(this, ticketDetails, ticketDisplayUnavailable ? new Models.Watch() : watch, false);
        TicketViews.addCatalog(this, ticketDetails, watch.priceRanges);
        if (!watch.demo) {
            VenueStore.Preset preset = new VenueStore(this).selected(watch);
            if (preset != null) {
                TextView reference = text("Venue reference: " + preset.name, 13);
                reference.setTextColor(TEAL);
                add(ticketDetails, reference, 12);
                if (preset.sectionReferences != null && !preset.sectionReferences.isEmpty()) {
                    TextView sections = text("Sections: " + android.text.TextUtils.join(" · ", preset.sectionReferences)
                            + "\nSaved references; availability is checked for this event.", 12);
                    sections.setTextColor(MUTED);
                    add(ticketDetails, sections, 5);
                }
            }
            Button presets = button("Venue presets", false);
            presets.setId(R.id.ui_venue_presets);
            presets.setOnClickListener(view -> {
                Intent intent = new Intent(this, VenueActivity.class);
                intent.putExtra("watch_id", watch.id);
                startActivity(intent);
            });
            add(ticketDetails, presets, 12);
        }
    }

    private void showStatus(String state, String detail) {
        String label;
        if ("CHECKING".equals(state)) label = "CHECKING PAGE…";
        else if ("AVAILABLE".equals(state)) label = "AVAILABLE · SIGNAL OBSERVED";
        else if ("UNAVAILABLE".equals(state)) label = "UNAVAILABLE · SIGNAL OBSERVED";
        else if ("BLOCKED".equals(state)) label = "BLOCKED · BROWSER ACTION NEEDED";
        else if ("DEMO PREVIEW".equals(state)) label = "DEMO · SYNTHETIC EVENT";
        else label = "UNKNOWN · AVAILABILITY UNVERIFIED";
        stateLabel.setText(label);
        stateLabel.setTextColor("AVAILABLE".equals(state) || "CHECKING".equals(state) ? TEAL : AMBER);
        status.setText(detail);
        checkedLabel.setText(watch.demo ? "Demo data · not a live check" : lastChecked());
        check.setEnabled(supported && !rendererUnavailable && !"CHECKING".equals(state));
        check.setText("CHECKING".equals(state) ? "Checking…" : "Check page");
    }

    private String lastChecked() {
        return watch.checkedAt > 0 ? "Last checked " + DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.MEDIUM).format(new Date(watch.checkedAt)) : "Not checked yet";
    }

    private boolean valid(int token) { return active && !destroyed && !isFinishing() && token == generation; }

    private void removeScheduled() {
        if (timeout != null) handler.removeCallbacks(timeout);
        if (inspection != null) handler.removeCallbacks(inspection);
        timeout = null;
        inspection = null;
    }

    private void cancelCycle() { generation++; removeScheduled(); }

    private static boolean sameUrl(String first, String second) {
        if (first == null || second == null) return false;
        // Fragment changes do not change the loaded page; path and query changes do.
        return first.split("#", 2)[0].equals(second.split("#", 2)[0]);
    }

    private void openExternal(String url) {
        if (!PageSignals.isHttps(url)) {
            Toast.makeText(this, "Only HTTPS event links can open.", Toast.LENGTH_LONG).show();
            return;
        }
        try { startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)); }
        catch (ActivityNotFoundException error) { Toast.makeText(this, "No browser is installed to open this link.", Toast.LENGTH_LONG).show(); }
    }

    @Override protected void onStart() {
        super.onStart();
        active = true;
        if (initialLoad) {
            Models.Watch latest = store.get(watchId);
            if (latest == null) { finish(); return; }
            watch = latest;
            renderStatus();
        }
        if (web == null) return;
        web.onResume();
        if (!initialLoad) {
            initialLoad = true;
            if (supported) {
                startCycle(watch.url, true);
                web.loadUrl(watch.url);
            } else if (!watch.demo) openExternal(watch.url);
        } else if (supported && interrupted) {
            interrupted = false;
            failed = true;
            failureState = "UNKNOWN";
            settled = true;
            record("UNKNOWN", "The previous check stopped when this screen left the foreground. Tap Check page now to try again.");
        } else if (supported) {
            Models.Watch latest = store.get(watchId);
            if (latest == null) { finish(); return; }
            watch = latest;
            renderStatus();
        }
    }

    @Override protected void onStop() {
        interrupted = supported && !settled;
        active = false;
        cancelCycle();
        if (web != null) { web.stopLoading(); web.onPause(); }
        super.onStop();
    }

    @Override protected void onDestroy() {
        destroyed = true;
        active = false;
        cancelCycle();
        if (web != null) {
            web.stopLoading();
            web.setWebChromeClient(null);
            web.setWebViewClient(new WebViewClient());
            ((android.view.ViewGroup) web.getParent()).removeView(web);
            web.destroy();
            web = null;
        }
        super.onDestroy();
    }

    private TextView text(String value, int size) {
        TextView view = new TextView(this);
        view.setText(value);
        view.setTextSize(size);
        view.setTextColor(TEXT);
        return view;
    }

    private LinearLayout column() {
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        return layout;
    }

    private void add(LinearLayout parent, View child, int topMargin) {
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
        params.topMargin = dp(topMargin);
        parent.addView(child, params);
    }

    private GradientDrawable surface(int fill, int stroke, int radius) {
        GradientDrawable drawable = new GradientDrawable();
        drawable.setColor(fill);
        drawable.setCornerRadius(dp(radius));
        if (stroke != Color.TRANSPARENT) drawable.setStroke(dp(1), stroke);
        return drawable;
    }

    private StateListDrawable buttonBackground(boolean primary) {
        StateListDrawable states = new StateListDrawable();
        states.addState(new int[] { -android.R.attr.state_enabled }, surface(Color.rgb(24, 41, 56), BORDER, 12));
        states.addState(new int[] { android.R.attr.state_pressed }, surface(primary ? Color.rgb(63, 188, 165) : BORDER, Color.TRANSPARENT, 12));
        states.addState(new int[0], surface(primary ? TEAL : CARD, primary ? Color.TRANSPARENT : BORDER, 12));
        return states;
    }

    private Button button(String label, boolean primary) {
        Button button = new Button(this);
        button.setText(label);
        button.setAllCaps(false);
        button.setTextSize(14);
        button.setTypeface(null, Typeface.BOLD);
        button.setMinHeight(dp(48));
        button.setMinimumHeight(dp(48));
        button.setMinWidth(0);
        button.setPadding(dp(10), dp(10), dp(10), dp(10));
        button.setTextColor(new ColorStateList(new int[][] { new int[] { -android.R.attr.state_enabled }, new int[0] },
                new int[] { MUTED, primary ? BG : TEXT }));
        button.setBackground(buttonBackground(primary));
        return button;
    }

    private EditText selectorInput(String value) {
        EditText input = new EditText(this);
        input.setId(View.generateViewId());
        input.setText(value);
        input.setHint("No selector configured");
        input.setTextColor(TEXT);
        input.setHintTextColor(MUTED);
        input.setTextSize(13);
        input.setTypeface(Typeface.MONOSPACE);
        input.setMinHeight(dp(48));
        input.setPadding(dp(12), dp(12), dp(12), dp(12));
        input.setBackground(surface(BG, BORDER, 10));
        input.setSingleLine(true);
        input.setInputType(android.text.InputType.TYPE_CLASS_TEXT | android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        input.setFilters(new InputFilter[] { new InputFilter.LengthFilter(2000) });
        return input;
    }

    private int dp(int value) { return Math.round(value * getResources().getDisplayMetrics().density); }
}
