package app.ticketwatch.mobile;

import org.json.JSONArray;
import org.json.JSONObject;

import java.math.BigDecimal;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Currency;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;

/** Portable selection preferences. Saving them never authorizes a live reservation. */
public final class TicketPreferences {
    public static final int SCHEMA_VERSION = 1;
    public int quantity = 2;
    public Long maxTotalMinor;
    public String currency = "CZK";
    public boolean requireAdjacent = true;
    public String priceStrategy = "cheapest";
    public List<String> allowedCategoryIds = new ArrayList<>();
    public List<String> allowedTicketTypes = new ArrayList<>();
    public List<String> allowedSections = new ArrayList<>();
    public List<String> allowedRows = new ArrayList<>();
    private static final String[] FIELDS = {"quantity", "max_total_minor", "currency", "require_adjacent",
            "price_strategy", "allowed_category_ids", "allowed_ticket_types", "allowed_sections", "allowed_rows"};

    public JSONObject toJson() {
        try {
            JSONObject value = new JSONObject();
            value.put("quantity", quantity);
            value.put("max_total_minor", maxTotalMinor == null ? JSONObject.NULL : maxTotalMinor);
            value.put("currency", currency);
            value.put("require_adjacent", requireAdjacent);
            value.put("price_strategy", priceStrategy);
            value.put("allowed_category_ids", new JSONArray(allowedCategoryIds));
            value.put("allowed_ticket_types", new JSONArray(allowedTicketTypes));
            value.put("allowed_sections", new JSONArray(allowedSections));
            value.put("allowed_rows", new JSONArray(allowedRows));
            return value;
        } catch (Exception error) { throw new IllegalArgumentException("Ticket preferences are incomplete."); }
    }

    public static TicketPreferences fromJson(JSONObject value) {
        try {
            requireFields(value, FIELDS);
            TicketPreferences result = new TicketPreferences();
            result.quantity = integer(value.get("quantity"), "Ticket quantity").intValueExact();
            if (result.quantity < 1 || result.quantity > 20) throw new IllegalArgumentException("Choose between 1 and 20 tickets.");
            Object budget = value.get("max_total_minor");
            if (budget != JSONObject.NULL) {
                result.maxTotalMinor = integer(budget, "Total budget").longValueExact();
                if (result.maxTotalMinor <= 0 || result.maxTotalMinor > 9007199254740991L)
                    throw new IllegalArgumentException("Enter a positive total budget within the supported range.");
            }
            result.currency = requiredString(value.get("currency"), "Currency");
            currencyDigits(result.currency);
            if (!(value.get("require_adjacent") instanceof Boolean)) throw new IllegalArgumentException("Choose an adjacency preference.");
            result.requireAdjacent = value.getBoolean("require_adjacent");
            result.priceStrategy = requiredString(value.get("price_strategy"), "Price strategy");
            if (!"cheapest".equals(result.priceStrategy)) throw new IllegalArgumentException("Only cheapest eligible tickets are supported.");
            result.allowedCategoryIds = readList(value.get("allowed_category_ids"), true);
            result.allowedTicketTypes = readList(value.get("allowed_ticket_types"), false);
            result.allowedSections = readList(value.get("allowed_sections"), false);
            result.allowedRows = readList(value.get("allowed_rows"), false);
            return result;
        } catch (IllegalArgumentException error) { throw error; }
        catch (Exception error) { throw new IllegalArgumentException("Ticket preferences are invalid or incomplete."); }
    }

    public TicketPreferences checkedCopy() { return fromJson(toJson()); }

    /** Money uses the currency's minor units, never floating-point arithmetic. */
    public static Long parseBudget(String input, String currency) {
        int digits = currencyDigits(currency);
        String amount = input == null ? "" : input.trim();
        if (amount.isEmpty()) return null;
        if (!amount.matches("[0-9]+(?:[.,][0-9]+)?"))
            throw new IllegalArgumentException("Enter a total budget using digits and one decimal separator.");
        try {
            long minor = new BigDecimal(amount.replace(',', '.')).movePointRight(digits).longValueExact();
            if (minor <= 0 || minor > 9007199254740991L) throw new ArithmeticException();
            return minor;
        } catch (ArithmeticException error) {
            throw new IllegalArgumentException("Enter a positive budget with at most " + digits + " decimal places.");
        }
    }

    public String budgetText() {
        return maxTotalMinor == null ? "" : BigDecimal.valueOf(maxTotalMinor, currencyDigits(currency)).stripTrailingZeros().toPlainString();
    }

    public String summary() {
        String result = quantity + (quantity == 1 ? " ticket" : " tickets") + " · Cheapest";
        if (quantity > 1) result += requireAdjacent ? " · Adjacent" : " · Separate seats allowed";
        if (maxTotalMinor != null) result += " · " + budgetText() + " " + currency + " total limit";
        return result;
    }

    public static List<String> lines(String input) {
        return lines(input, false);
    }

    public static List<String> lines(String input, boolean caseSensitive) {
        JSONArray values = new JSONArray();
        for (String line : (input == null ? "" : input).split("\\r?\\n")) {
            String clean = line.trim();
            if (!clean.isEmpty()) values.put(clean);
        }
        return readList(values, caseSensitive);
    }

    static void requireFields(JSONObject value, String... fields) {
        if (value == null) throw new IllegalArgumentException("Preferences are missing.");
        Set<String> expected = new HashSet<>(Arrays.asList(fields));
        java.util.Iterator<String> names = value.keys();
        while (names.hasNext()) if (!expected.contains(names.next())) throw new IllegalArgumentException("Unsupported preference field.");
        for (String field : fields) if (!value.has(field)) throw new IllegalArgumentException("Preference field is missing: " + field + ".");
    }

    private static BigDecimal integer(Object value, String label) {
        if (!(value instanceof Number)) throw new IllegalArgumentException(label + " must be a number.");
        return new BigDecimal(value.toString());
    }

    private static String requiredString(Object value, String label) {
        if (!(value instanceof String)) throw new IllegalArgumentException(label + " must be text.");
        return (String) value;
    }

    private static int currencyDigits(String currency) {
        try {
            if (currency == null || !currency.matches("[A-Z]{3}")) throw new IllegalArgumentException();
            boolean known = false;
            for (Currency candidate : Currency.getAvailableCurrencies())
                if (currency.equals(candidate.getCurrencyCode())) { known = true; break; }
            if (!known) throw new IllegalArgumentException();
            int digits = Currency.getInstance(currency).getDefaultFractionDigits();
            if (digits < 0 || digits > 3) throw new IllegalArgumentException();
            return digits;
        } catch (IllegalArgumentException error) { throw new IllegalArgumentException("Enter a supported three-letter currency, such as CZK or EUR."); }
    }

    private static List<String> readList(Object value, boolean caseSensitive) {
        if (!(value instanceof JSONArray)) throw new IllegalArgumentException("Ticket filters must be lists.");
        JSONArray entries = (JSONArray) value;
        if (entries.length() > 100) throw new IllegalArgumentException("Use at most 100 entries in each ticket filter.");
        List<String> result = new ArrayList<>();
        Set<String> seen = new HashSet<>();
        for (int index = 0; index < entries.length(); index++) {
            String entry = requiredString(entries.opt(index), "Ticket filter");
            boolean control = false;
            for (int offset = 0; offset < entry.length();) {
                int codepoint = entry.codePointAt(offset);
                if (Character.isISOControl(codepoint)) control = true;
                offset += Character.charCount(codepoint);
            }
            String labelKey = normalizedLabel(entry);
            if (entry.isEmpty() || !entry.equals(entry.trim()) || entry.getBytes(StandardCharsets.UTF_8).length > 128
                    || control || labelKey.isEmpty())
                throw new IllegalArgumentException("Use one label per line, up to 128 bytes each.");
            if (caseSensitive && !entry.matches("[A-Za-z0-9._:-]+"))
                throw new IllegalArgumentException("Category IDs may contain letters, digits, dots, underscores, colons and hyphens.");
            String key = caseSensitive ? entry : labelKey;
            if (seen.add(key)) result.add(entry);
        }
        return result;
    }

    private static String normalizedLabel(String value) {
        StringBuilder result = new StringBuilder();
        boolean pendingSpace = false;
        for (int offset = 0; offset < value.length();) {
            int codepoint = value.codePointAt(offset);
            offset += Character.charCount(codepoint);
            if (Character.isWhitespace(codepoint) || Character.isSpaceChar(codepoint)) {
                pendingSpace = result.length() > 0;
            } else {
                if (pendingSpace) result.append(' ');
                result.appendCodePoint(codepoint);
                pendingSpace = false;
            }
        }
        return result.toString().toLowerCase(Locale.ROOT);
    }
}
