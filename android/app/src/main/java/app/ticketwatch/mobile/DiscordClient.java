package app.ticketwatch.mobile;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.InputStream;
import java.io.OutputStream;
import java.net.URL;
import java.nio.charset.StandardCharsets;

import javax.net.ssl.HttpsURLConnection;

/** Sends only to Discord's official API; tokens stay in the Authorization header. */
public final class DiscordClient {
    private DiscordClient() { }

    public static void sendTest(String token, String channelId, String userId) throws Exception {
        validate(token, channelId, userId);
        JSONObject payload = base(userId, "Ticket Watch test message — no availability change was detected.");
        send(token, channelId, payload);
    }

    public static void sendAlert(String token, String channelId, String userId, Models.Watch watch) throws Exception {
        JSONObject payload = buildAlertPayload(userId, watch);
        validate(token, channelId, userId);
        send(token, channelId, payload);
    }

    /** Pure payload builder: usable by offline tests without a token or network. */
    public static JSONObject buildAlertPayload(String userId, Models.Watch watch) throws Exception {
        if (watch == null || watch.demo) throw new Exception("Demo observations cannot send live Discord alerts.");
        if (!watch.selectorsVerified || !"AVAILABLE".equals(watch.state) || !"AVAILABLE".equals(watch.lastDefinite)) {
            throw new Exception("A verified available page observation is required for an alert.");
        }
        String publicUrl = DiscoveryClient.normalizePublicUrl(watch.url);
        if (publicUrl.isEmpty()) throw new Exception("The watch has no supported public HTTPS link.");
        if (!Models.safe(userId).trim().isEmpty() && !userId.trim().matches("[0-9]{15,22}"))
            throw new Exception("Enter a valid Discord user ID, or leave the mention empty.");
        JSONObject payload = base(userId, "Ticket Watch observed an unavailable → available change on the event page.");
        JSONObject embed = new JSONObject();
        embed.put("title", clipped(watch.name, 256, "Ticket availability observed"));
        embed.put("url", publicUrl);
        embed.put("color", 0x42c99a);
        embed.put("description", "A verified page selector indicates availability. Open the official event page to confirm current offers.");
        JSONArray fields = new JSONArray();
        addField(fields, "Artist", watch.artist);
        addField(fields, "Date", watch.date);
        addField(fields, "Location", join(watch.city, watch.country));
        addField(fields, "Venue", watch.venue);
        fields.put(new JSONObject().put("name", "Ticket details source")
                .put("value", TicketOfferText.observation(watch)).put("inline", false));
        if (TicketOfferText.hasCurrentOffers(watch)) {
            int shown = Math.min(3, watch.ticketSnapshot.offers.size());
            for (int i = 0; i < shown; i++) {
                fields.put(new JSONObject().put("name", "Observed offer " + (i + 1))
                        .put("value", TicketOfferText.offer(watch.ticketSnapshot.offers.get(i))).put("inline", false));
            }
            if (watch.ticketSnapshot.offers.size() > shown) fields.put(new JSONObject().put("name", "More offers")
                    .put("value", "Additional observed offers are shown in the app. This is not an event-wide stock total.").put("inline", false));
        }
        embed.put("fields", fields);
        embed.put("footer", new JSONObject().put("text", "Page observation only · no reservation or checkout · Discovery status is not inventory"));
        payload.put("embeds", new JSONArray().put(embed));
        return payload;
    }

    private static JSONObject base(String userId, String message) throws Exception {
        String user = Models.safe(userId).trim();
        JSONArray users = new JSONArray();
        if (!user.isEmpty()) users.put(user);
        JSONObject allowed = new JSONObject();
        allowed.put("parse", new JSONArray());
        allowed.put("users", users);
        allowed.put("roles", new JSONArray());
        allowed.put("replied_user", false);
        return new JSONObject().put("content", (user.isEmpty() ? "" : "<@" + user + "> ") + message)
                .put("allowed_mentions", allowed);
    }

    private static void addField(JSONArray fields, String name, String value) throws Exception {
        if (!Models.safe(value).trim().isEmpty()) {
            fields.put(new JSONObject().put("name", name).put("value", TicketOfferText.clean(value, 200, "Unknown")).put("inline", true));
        }
    }

    private static String join(String city, String country) {
        if (Models.safe(city).isEmpty()) return Models.safe(country);
        return city + (Models.safe(country).isEmpty() ? "" : ", " + country);
    }

    private static String clipped(String value, int maximum, String fallback) {
        String result = Models.safe(value).trim();
        if (result.isEmpty()) result = fallback;
        return result.length() <= maximum ? result : result.substring(0, maximum);
    }

    private static void validate(String token, String channelId, String userId) throws Exception {
        if (token == null || !token.trim().matches("[A-Za-z0-9._-]{10,512}")) {
            throw new Exception("Save a valid Discord bot token in Settings first.");
        }
        if (channelId == null || !channelId.trim().matches("[0-9]{15,22}")) throw new Exception("Enter a valid Discord channel ID.");
        if (!Models.safe(userId).trim().isEmpty() && !userId.trim().matches("[0-9]{15,22}")) {
            throw new Exception("Enter a valid Discord user ID, or leave the mention empty.");
        }
    }

    private static void send(String token, String channelId, JSONObject payload) throws Exception {
        HttpsURLConnection connection = null;
        try {
            byte[] body = payload.toString().getBytes(StandardCharsets.UTF_8);
            if (body.length > 32 * 1024) throw new DeliveryFailure("The Discord alert is too large.");
            URL url = new URL("https://discord.com/api/v10/channels/" + channelId.trim() + "/messages");
            connection = (HttpsURLConnection) url.openConnection();
            connection.setInstanceFollowRedirects(false);
            connection.setConnectTimeout(20000);
            connection.setReadTimeout(20000);
            connection.setUseCaches(false);
            connection.setRequestMethod("POST");
            connection.setRequestProperty("Authorization", "Bot " + token.trim());
            connection.setRequestProperty("Content-Type", "application/json; charset=utf-8");
            connection.setRequestProperty("Accept", "application/json");
            connection.setDoOutput(true);
            connection.setFixedLengthStreamingMode(body.length);
            try (OutputStream stream = connection.getOutputStream()) { stream.write(body); }
            int status = connection.getResponseCode();
            if (status == 429) throw new DeliveryFailure("Discord rate limited this message. Wait before sending another test.");
            if (status == 401 || status == 403) throw new DeliveryFailure("Discord refused delivery. Check the bot token and channel permissions.");
            if (status >= 300 && status < 400) throw new DeliveryFailure("Discord returned a redirect; it was not followed.");
            if (status < 200 || status >= 300) throw new DeliveryFailure("Discord did not accept the message. Check the channel ID and try again later.");
            // Do not retain the returned message, channel data, or response body.
            if (connection.getContentLengthLong() > 256 * 1024) throw new DeliveryFailure("Discord returned an unexpected response; verify delivery in the channel.");
            try (InputStream stream = connection.getInputStream()) {
                byte[] buffer = new byte[4096];
                int total = 0;
                int count;
                while ((count = stream.read(buffer)) != -1) {
                    total += count;
                    if (total > 256 * 1024) throw new DeliveryFailure("Discord returned an unexpected response; verify delivery in the channel.");
                }
            }
        } catch (DeliveryFailure safe) {
            throw safe;
        } catch (Exception ignored) {
            throw new Exception("Discord delivery could not be confirmed. Check the channel before sending again.");
        } finally {
            if (connection != null) connection.disconnect();
        }
    }

    private static final class DeliveryFailure extends Exception {
        DeliveryFailure(String message) { super(message); }
    }
}
