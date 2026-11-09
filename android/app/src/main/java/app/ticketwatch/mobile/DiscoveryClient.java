package app.ticketwatch.mobile;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.net.URI;
import java.net.URL;
import java.net.URLDecoder;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.text.Normalizer;
import java.text.SimpleDateFormat;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Date;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.TimeZone;

import javax.net.ssl.HttpsURLConnection;

/** Official Discovery API only. Discovery status is metadata, never ticket inventory. */
public final class DiscoveryClient {
    private static final String API_ROOT = "https://app.ticketmaster.com/discovery/v2/";
    private static final int MAX_BYTES = 4 * 1024 * 1024;
    private static final long DAY_MS = 24L * 60L * 60L * 1000L;
    private static final int DAILY_BUDGET = 4000;
    private static final Object REQUEST_LOCK = new Object();
    private static final Set<String> MARKETS = new HashSet<>(Arrays.asList(
            "ticketmaster.com", "ticketmaster.ca", "ticketmaster.co.uk", "ticketmaster.ie",
            "ticketmaster.pl", "ticketmaster.cz", "ticketmaster.de", "ticketmaster.at",
            "ticketmaster.ch", "ticketmaster.be", "ticketmaster.nl", "ticketmaster.fr",
            "ticketmaster.es", "ticketmaster.it", "ticketmaster.dk", "ticketmaster.no",
            "ticketmaster.se", "ticketmaster.fi", "ticketmaster.com.au", "ticketmaster.co.nz",
            "ticketmaster.com.mx", "ticketmaster.ae", "ticketmaster.co.za"));
    private final SharedPreferences preferences;

    public DiscoveryClient(Context context) {
        preferences = context.getApplicationContext().getSharedPreferences("discovery_limits", Context.MODE_PRIVATE);
    }

    public List<Models.Artist> searchArtists(String key, String query) throws Exception {
        validateKey(key);
        String search = input(query, 200);
        if (search.isEmpty()) throw new Exception("Enter an artist name.");
        Map<String, String> parameters = new LinkedHashMap<>();
        parameters.put("apikey", key.trim());
        parameters.put("keyword", search);
        parameters.put("classificationName", "music");
        parameters.put("size", "20");
        JSONObject root = request("attractions.json", parameters);
        List<Models.Artist> result = new ArrayList<>();
        JSONArray artists = resultItems(root, "attractions");
        Set<String> seen = new HashSet<>();
        for (int i = 0; i < Math.min(artists.length(), 100); i++) {
            JSONObject item = artists.optJSONObject(i);
            String id = text(item, "id", 150);
            String name = text(item, "name", 500);
            if (!id.matches("[A-Za-z0-9_-]{1,150}")) throw malformedResponse();
            if (!seen.add(id)) continue;
            Models.Artist artist = new Models.Artist();
            artist.id = id;
            artist.name = name;
            result.add(artist);
        }
        return result;
    }

    public Models.EventPage events(String key, String attractionId, String country, String city, int page) throws Exception {
        validateKey(key);
        if (attractionId == null || !attractionId.matches("[A-Za-z0-9_-]{1,150}")) {
            throw new Exception("Choose an artist from the search results.");
        }
        String requestedCountry = input(country, 2).toUpperCase(Locale.ROOT);
        if (!requestedCountry.isEmpty() && !requestedCountry.matches("[A-Z]{2}")) {
            throw new Exception("Use a two-letter country code, such as PL or US.");
        }
        String requestedCity = input(city, 120);
        if (page < 0 || page > 19) throw new Exception("This search has reached its 1,000-event limit. Narrow the location.");
        Map<String, String> parameters = new LinkedHashMap<>();
        parameters.put("apikey", key.trim());
        parameters.put("attractionId", attractionId);
        if (!requestedCountry.isEmpty()) parameters.put("countryCode", requestedCountry);
        if (!requestedCity.isEmpty()) parameters.put("city", requestedCity);
        parameters.put("page", String.valueOf(page));
        parameters.put("size", "50");
        parameters.put("sort", "date,asc");
        JSONObject root = request("events.json", parameters);
        return parseEvents(root, attractionId, requestedCountry, requestedCity);
    }

    static Models.EventPage parseEvents(JSONObject root, String attractionId, String requestedCountry, String requestedCity) throws Exception {
        JSONArray events = resultItems(root, "events");
        Models.EventPage result = new Models.EventPage();
        JSONObject paging = root.optJSONObject("page");
        result.page = pageNumber(paging, "number");
        result.totalPages = pageNumber(paging, "totalPages");
        result.totalElements = pageNumber(paging, "totalElements");
        Set<String> seen = new HashSet<>();
        for (int i = 0; i < Math.min(events.length(), 1000); i++) {
            JSONObject item = events.optJSONObject(i);
            if (!hasAttraction(item, attractionId)) continue;
            JSONObject eventEmbedded = item.optJSONObject("_embedded");
            JSONArray venues = eventEmbedded == null ? null : eventEmbedded.optJSONArray("venues");
            JSONObject venue = venues == null ? null : venues.optJSONObject(0);
            JSONObject locationCity = venue == null ? null : venue.optJSONObject("city");
            JSONObject locationCountry = venue == null ? null : venue.optJSONObject("country");
            String cityName = text(locationCity, "name", 200);
            String countryCode = text(locationCountry, "countryCode", 2).toUpperCase(Locale.ROOT);
            if (!requestedCountry.isEmpty() && !requestedCountry.equals(countryCode)) continue;
            if (!requestedCity.isEmpty() && !normalizedText(requestedCity).equals(normalizedText(cityName))) continue;
            Models.Event event = new Models.Event();
            event.id = text(item, "id", 150);
            event.url = normalizePublicUrl(text(item, "url", 4096));
            if (!seen.add(event.id)) continue;
            event.name = text(item, "name", 500);
            event.city = cityName;
            event.country = countryCode;
            event.venue = text(venue, "name", 500);
            event.venueId = text(venue, "id", 150);
            JSONObject dates = item.optJSONObject("dates");
            JSONObject start = dates == null ? null : dates.optJSONObject("start");
            JSONObject status = dates == null ? null : dates.optJSONObject("status");
            event.date = text(start, "localDate", 30);
            String time = text(start, "localTime", 30);
            if (!time.isEmpty()) event.date = (event.date + " " + time).trim();
            if (event.date.isEmpty()) event.date = text(start, "dateTime", 60);
            event.status = text(status, "code", 60);
            event.priceRanges = TicketData.readPriceRanges(item.optJSONArray("priceRanges"));
            result.events.add(event);
        }
        return result;
    }

    /** A missing/malformed payload must never masquerade as a successful empty search. */
    static JSONArray resultItems(JSONObject root, String key) throws Exception {
        if (root == null || root.has("fault") || root.has("error") || root.has("errors")) throw malformedResponse();
        JSONObject paging = root.optJSONObject("page");
        int count = pageNumber(paging, "totalElements");
        pageNumber(paging, "number");
        pageNumber(paging, "totalPages");
        JSONObject embedded = root.optJSONObject("_embedded");
        if (root.has("_embedded") && embedded == null) throw malformedResponse();
        JSONArray items = embedded == null ? null : embedded.optJSONArray(key);
        if (embedded != null && embedded.has(key) && items == null) throw malformedResponse();
        if (items == null) {
            if (count == 0) return new JSONArray();
            throw malformedResponse();
        }
        if ((count == 0 && items.length() != 0) || (count > 0 && items.length() == 0)) throw malformedResponse();
        for (int i = 0; i < items.length(); i++) {
            JSONObject item = items.optJSONObject(i);
            if (item == null || !(item.opt("id") instanceof String) || !(item.opt("name") instanceof String)
                    || text(item, "id", 150).trim().isEmpty() || text(item, "name", 500).trim().isEmpty()) {
                throw malformedResponse();
            }
        }
        return items;
    }

    private static int pageNumber(JSONObject page, String key) throws DiscoveryFailure {
        Object value = page == null ? null : page.opt(key);
        if (!(value instanceof Number)) throw malformedResponse();
        double number = ((Number) value).doubleValue();
        if (!Double.isFinite(number) || number < 0 || number > Integer.MAX_VALUE || number != Math.floor(number)) {
            throw malformedResponse();
        }
        return (int) number;
    }

    private static DiscoveryFailure malformedResponse() {
        return new DiscoveryFailure("Discovery returned an unexpected response. Search completeness is unknown; try again later.");
    }

    private static boolean hasAttraction(JSONObject event, String id) {
        JSONObject embedded = event.optJSONObject("_embedded");
        JSONArray attractions = embedded == null ? null : embedded.optJSONArray("attractions");
        if (attractions == null) return false;
        for (int i = 0; i < attractions.length(); i++) {
            JSONObject attraction = attractions.optJSONObject(i);
            if (id.equals(text(attraction, "id", 150))) return true;
        }
        return false;
    }

    private JSONObject request(String endpoint, Map<String, String> parameters) throws Exception {
        HttpsURLConnection connection = null;
        try {
            StringBuilder query = new StringBuilder();
            for (Map.Entry<String, String> entry : parameters.entrySet()) {
                if (query.length() > 0) query.append('&');
                query.append(URLEncoder.encode(entry.getKey(), "UTF-8")).append('=')
                        .append(URLEncoder.encode(entry.getValue(), "UTF-8"));
            }
            reserveAttempt();
            connection = (HttpsURLConnection) new URL(API_ROOT + endpoint + "?" + query).openConnection();
            connection.setInstanceFollowRedirects(false);
            connection.setConnectTimeout(20000);
            connection.setReadTimeout(20000);
            connection.setUseCaches(false);
            connection.setRequestMethod("GET");
            connection.setRequestProperty("Accept", "application/json");
            int status = connection.getResponseCode();
            if (status == 429) {
                saveCooldown(connection.getHeaderField("Retry-After"));
                throw new DiscoveryFailure("Discovery rate limit reached. Wait for the provider cooldown before trying again.");
            }
            if (status == 401 || status == 403) throw new DiscoveryFailure("Discovery access was refused. Check the API key and its permissions.");
            if (status >= 300 && status < 400) throw new DiscoveryFailure("Discovery returned a redirect; it was not followed.");
            if (status != 200) throw new DiscoveryFailure("Discovery is unavailable or rejected the search. Try again later.");
            if (connection.getContentLengthLong() > MAX_BYTES) throw new DiscoveryFailure("Discovery returned too much data. Narrow the search.");
            try (InputStream stream = connection.getInputStream(); ByteArrayOutputStream bytes = new ByteArrayOutputStream()) {
                byte[] buffer = new byte[8192];
                int count;
                while ((count = stream.read(buffer)) != -1) {
                    if (bytes.size() + count > MAX_BYTES) throw new DiscoveryFailure("Discovery returned too much data. Narrow the search.");
                    bytes.write(buffer, 0, count);
                }
                return new JSONObject(new String(bytes.toByteArray(), StandardCharsets.UTF_8));
            }
        } catch (DiscoveryFailure safe) {
            throw safe;
        } catch (InterruptedException ignored) {
            Thread.currentThread().interrupt();
            throw new Exception("The Discovery request was interrupted.");
        } catch (Exception ignored) {
            // Never attach the original exception: its message can contain the full API URL/key.
            throw new Exception("Discovery could not be read. Check the connection and try again.");
        } finally {
            if (connection != null) connection.disconnect();
        }
    }

    /** Counts attempted requests, including failures, in a persisted rolling 24-hour window. */
    private void reserveAttempt() throws Exception {
        synchronized (REQUEST_LOCK) {
            long now = System.currentTimeMillis();
            if (preferences.getLong("cooldown_until", 0) > now) {
                throw new DiscoveryFailure("Discovery is cooling down after a rate limit. Try again later.");
            }
            long last = preferences.getLong("last_attempt", 0);
            if (last > now) throw new DiscoveryFailure("Device time changed. Correct the clock before searching again.");
            long wait = 500 - (now - last);
            if (wait > 0) Thread.sleep(wait);
            now = System.currentTimeMillis();
            JSONArray prior = new JSONArray(preferences.getString("attempts", "[]"));
            JSONArray recent = new JSONArray();
            for (int i = 0; i < prior.length(); i++) {
                long timestamp = prior.getLong(i);
                if (timestamp > now - DAY_MS) recent.put(timestamp);
            }
            if (recent.length() >= DAILY_BUDGET) {
                throw new DiscoveryFailure("The local 4,000-request rolling daily budget is exhausted. Try again after earlier requests expire.");
            }
            recent.put(now);
            if (!preferences.edit().putString("attempts", recent.toString()).putLong("last_attempt", now).commit()) {
                throw new DiscoveryFailure("The Discovery request budget could not be saved. Try again.");
            }
        }
    }

    private void saveCooldown(String retryAfter) throws DiscoveryFailure {
        synchronized (REQUEST_LOCK) {
            long now = System.currentTimeMillis();
            long delay = 60_000;
            if (retryAfter != null && retryAfter.length() < 100) {
                try {
                    double seconds = Double.parseDouble(retryAfter.trim());
                    if (Double.isFinite(seconds) && seconds >= 0) delay = (long) Math.min(seconds * 1000, 7 * DAY_MS);
                } catch (Exception ignored) {
                    try {
                        SimpleDateFormat format = new SimpleDateFormat("EEE, dd MMM yyyy HH:mm:ss zzz", Locale.US);
                        format.setTimeZone(TimeZone.getTimeZone("GMT"));
                        format.setLenient(false);
                        Date date = format.parse(retryAfter);
                        if (date != null) delay = date.getTime() - now;
                    } catch (Exception ignoredDate) { }
                }
            }
            long until = now + Math.max(1000, Math.min(delay, 7 * DAY_MS));
            until = Math.max(until, preferences.getLong("cooldown_until", 0));
            if (!preferences.edit().putLong("cooldown_until", until).commit()) {
                throw new DiscoveryFailure("Discovery was rate limited, but the cooldown could not be saved. Wait before trying again.");
            }
        }
    }

    /** Returns a canonical public link or an empty string. Query strings are never retained. */
    static boolean isTicketmasterSite(String site) { return MARKETS.contains(site); }

    public static String normalizePublicUrl(String value) {
        if (value == null || value.length() > 4096) return "";
        try {
            URI uri = new URI(value.trim());
            if (!"https".equalsIgnoreCase(uri.getScheme()) || uri.getRawUserInfo() != null
                    || (uri.getPort() != -1 && uri.getPort() != 443)) return "";
            String host = uri.getHost();
            if (host == null) return "";
            host = host.toLowerCase(Locale.ROOT);
            String bare = host.startsWith("www.") ? host.substring(4) : host;
            boolean ticketmaster = MARKETS.contains(bare);
            boolean frontgate = host.equals("on.fgtix.com") || host.equals("frontgatetickets.com")
                    || host.endsWith(".frontgatetickets.com");
            if (!ticketmaster && !frontgate) return "";
            String query = uri.getRawQuery();
            if (query != null) {
                for (String pair : query.split("[&;]")) {
                    String name = URLDecoder.decode(pair.split("=", 2)[0], "UTF-8").toLowerCase(Locale.ROOT);
                    if (name.equals("apikey") || name.equals("api_key") || name.equals("token")
                            || name.equals("session") || name.equals("sessionid") || name.equals("jsessionid")
                            || name.equals("jwt") || name.equals("t") || name.equals("access_token")
                            || name.equals("refresh_token") || name.equals("password") || name.equals("secret")) return "";
                }
            }
            String path = uri.getRawPath();
            if (path == null || path.isEmpty()) path = "/";
            String lower = path.toLowerCase(Locale.ROOT);
            if (lower.contains("%2f") || lower.contains("%5c") || lower.contains("%25") || lower.contains("%00")
                    || lower.contains(";") || lower.contains("\\")) return "";
            String decoded = URLDecoder.decode(path, "UTF-8").toLowerCase(Locale.ROOT);
            if (hasControls(decoded) || decoded.contains(";")) return "";
            for (String segment : decoded.split("/")) {
                if (segment.equals(".") || segment.equals("..") || segment.equals("checkout") || segment.equals("payment") || segment.equals("payments")
                        || segment.equals("cart") || segment.equals("basket") || segment.equals("account")
                        || segment.equals("myaccount") || segment.equals("login") || segment.equals("signin")
                        || segment.equals("session") || segment.equals("sessions") || segment.equals("orders")) return "";
            }
            // Bare and www Ticketmaster links share an identity. Front Gate subdomains are distinct stores.
            if (ticketmaster) host = "www." + bare;
            return "https://" + host + path;
        } catch (Exception ignored) {
            return "";
        }
    }

    private static void validateKey(String key) throws Exception {
        if (key == null || !key.trim().matches("[A-Za-z0-9._-]{8,512}")) {
            throw new Exception("Save a valid Discovery API consumer key in Settings first.");
        }
    }

    private static String input(String value, int limit) throws Exception {
        String result = Models.safe(value).trim();
        if (result.length() > limit || hasControls(result)) throw new Exception("The search input is not valid.");
        return result;
    }

    private static boolean hasControls(String value) {
        for (int i = 0; i < value.length(); i++) {
            char character = value.charAt(i);
            if (character <= 0x1f || character == 0x7f) return true;
        }
        return false;
    }

    private static String normalizedText(String value) {
        return Normalizer.normalize(value, Normalizer.Form.NFKC).trim().replaceAll("\\s+", " ").toLowerCase(Locale.ROOT);
    }

    private static String text(JSONObject object, String key, int limit) {
        if (object == null || object.isNull(key)) return "";
        String value = object.optString(key, "");
        return value.length() > limit ? value.substring(0, limit) : value;
    }

    private static final class DiscoveryFailure extends Exception {
        DiscoveryFailure(String message) { super(message); }
    }
}
