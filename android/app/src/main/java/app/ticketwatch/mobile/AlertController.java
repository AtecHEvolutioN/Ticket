package app.ticketwatch.mobile;

import android.Manifest;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.widget.Toast;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Notification delivery is separate from the page's inventory decision. */
public final class AlertController {
    private static final String CHANNEL = "ticket_availability";
    private static final ExecutorService DELIVERY = Executors.newSingleThreadExecutor();
    private AlertController() {}

    public static void notifyAvailability(Context context, Models.Watch watch) {
        if (watch == null) return;
        if (!watch.demo && (!watch.selectorsVerified || !"AVAILABLE".equals(watch.state))) return;
        final Context app = context.getApplicationContext();
        NotificationManager manager = (NotificationManager) app.getSystemService(Context.NOTIFICATION_SERVICE);
        NotificationChannel channel = new NotificationChannel(CHANNEL, "Ticket alerts", NotificationManager.IMPORTANCE_HIGH);
        channel.setDescription("Verified page changes and clearly labelled demo alerts.");
        manager.createNotificationChannel(channel);
        String title = watch.demo ? "Demo ticket alert" : "Tickets detected: " + watch.name;
        String detail = TicketOfferText.notificationDetail(watch);
        Intent target = new Intent(app, watch.demo ? MainActivity.class : PageActivity.class);
        target.putExtra("watch_id", watch.id);
        PendingIntent tap = PendingIntent.getActivity(app, watch.id.hashCode(), target,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        boolean permitted = Build.VERSION.SDK_INT < 33 || app.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED;
        if (permitted) {
            Notification notification = new Notification.Builder(app, CHANNEL)
                    .setSmallIcon(R.drawable.ic_notification).setContentTitle(title)
                    .setContentText(detail).setStyle(new Notification.BigTextStyle().bigText(detail))
                    .setContentIntent(tap).setAutoCancel(true).setCategory(Notification.CATEGORY_EVENT)
                    .setVisibility(Notification.VISIBILITY_PRIVATE).build();
            manager.notify(watch.id.hashCode(), notification);
        }
        toast(app, permitted ? title : title + " — allow notifications to receive phone pings");
        if (watch.demo) return;
        SharedPreferences prefs = app.getSharedPreferences("ticket_settings", Context.MODE_PRIVATE);
        if (!prefs.getBoolean("discord_enabled", false)) return;
        final String channelId = prefs.getString("discord_channel", "");
        final String userId = prefs.getString("discord_user", "");
        DELIVERY.execute(() -> {
            try {
                String token = new SecureStore(app).get("discord_token");
                if (token == null || token.isEmpty() || channelId.isEmpty()) {
                    toast(app, "Discord is enabled but needs its token and channel in Settings.");
                    return;
                }
                DiscordClient.sendAlert(token, channelId, userId, watch);
                toast(app, "Discord accepted the ticket alert");
            } catch (Exception ignored) {
                toast(app, "Discord delivery failed. Check the destination and token in Settings.");
            }
        });
    }

    private static void toast(Context context, String text) {
        new Handler(Looper.getMainLooper()).post(() -> Toast.makeText(context, text, Toast.LENGTH_LONG).show());
    }
}
