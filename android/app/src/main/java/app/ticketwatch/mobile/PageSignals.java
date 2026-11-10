package app.ticketwatch.mobile;

import java.net.URI;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Locale;
import java.util.Set;

/** Conservative, platform-independent rules for public event page observations. */
final class PageSignals {
    private static final Set<String> ROOTS = new HashSet<>(Arrays.asList(
            "ticketmaster.com", "ticketmaster.ca", "ticketmaster.co.uk", "ticketmaster.ie",
            "ticketmaster.de", "ticketmaster.fr", "ticketmaster.es", "ticketmaster.it",
            "ticketmaster.nl", "ticketmaster.be", "ticketmaster.at", "ticketmaster.ch",
            "ticketmaster.dk", "ticketmaster.se", "ticketmaster.no", "ticketmaster.fi",
            "ticketmaster.pl", "ticketmaster.cz", "ticketmaster.co.nz", "ticketmaster.com.au",
            "ticketmaster.com.mx", "ticketmaster.com.br", "ticketmaster.cl", "ticketmaster.sg",
            "ticketmaster.ae", "ticketmaster.co.za"));

    private PageSignals() { }

    static boolean isHttps(String value) {
        try {
            URI uri = new URI(value);
            return "https".equalsIgnoreCase(uri.getScheme()) && uri.getHost() != null
                    && uri.getRawUserInfo() == null && (uri.getPort() == -1 || uri.getPort() == 443);
        } catch (Exception ignored) { return false; }
    }

    static boolean isTicketmasterUrl(String value) {
        if (!isHttps(value)) return false;
        try {
            String host = new URI(value).getHost().toLowerCase(Locale.ROOT);
            // Developer/API hosts are not public browsing destinations.
            if (host.startsWith("app.") || host.startsWith("developer.") || host.startsWith("api.")) return false;
            for (String root : ROOTS) {
                if (host.equals(root) || host.endsWith("." + root)) return true;
            }
        } catch (Exception ignored) { }
        return false;
    }

    static boolean sameEvent(String expected, String actual) {
        if (!isTicketmasterUrl(expected) || !isTicketmasterUrl(actual)) return false;
        try {
            URI first = new URI(expected);
            URI second = new URI(actual);
            String expectedId = eventId(first);
            return !expectedId.isEmpty() && expectedId.equals(eventId(second))
                    && publicHost(first).equals(publicHost(second));
        } catch (Exception ignored) { return false; }
    }

    private static String publicHost(URI uri) {
        String host = uri.getHost().toLowerCase(Locale.ROOT);
        return host.startsWith("www.") ? host.substring(4) : host;
    }

    private static String eventId(URI uri) {
        try {
            String query=uri.getRawQuery();
            if(query!=null) for(String item:query.split("&")) {
                String key=item.split("=",2)[0];
                if(!"language".equals(key)&&!"locale".equals(key)&&!"page".equals(key))return "";
            }
            String path=uri.getRawPath();
            if(path==null||!path.startsWith("/")||path.contains("//")||path.indexOf('\\')>=0)return "";
            if(path.endsWith("/"))path=path.substring(0,path.length()-1);
            String[] raw=path.substring(1).split("/",-1), decoded=new String[raw.length];
            for(int i=0;i<raw.length;i++){decoded[i]=decodeSegment(raw[i]);if(decoded[i]==null)return "";}
            boolean shape=raw.length==2&&"event".equals(raw[0]);
            if(raw.length==3)shape=("event".equals(raw[0])&&safeSlug(decoded[1]))
                    ||("event".equals(raw[1])&&safeSlug(decoded[0]));
            if(raw.length==4)shape=(locale(raw[0])&&"event".equals(raw[1])&&safeSlug(decoded[2]))
                    ||(locale(raw[0])&&safeSlug(decoded[1])&&"event".equals(raw[2]))
                    ||("ticketmaster.it".equals(publicHost(uri))&&"biglietti".equals(raw[0])&&safeSlug(decoded[1])&&"event".equals(raw[2]));
            if(raw.length==5)shape="ticketmaster.it".equals(publicHost(uri))&&locale(raw[0])&&"biglietti".equals(raw[1])&&safeSlug(decoded[2])&&"event".equals(raw[3]);
            if(!shape)return "";
            String id=raw[raw.length-1];
            return id.matches("[0-9]{1,20}")||(id.matches("[A-Za-z0-9]{8,64}")&&id.matches(".*[0-9].*")&&id.matches(".*[A-Za-z].*")) ? id.toUpperCase(Locale.ROOT) : "";
        }catch(Exception ignored){return "";}
    }

    private static String decodeSegment(String raw) throws Exception {
        java.io.ByteArrayOutputStream out=new java.io.ByteArrayOutputStream();
        byte[] bytes=raw.getBytes(java.nio.charset.StandardCharsets.UTF_8);
        for(int i=0;i<bytes.length;i++) {
            if(bytes[i]=='%') {
                if(i+2>=bytes.length)return null;
                int a=Character.digit((char)bytes[++i],16),b=Character.digit((char)bytes[++i],16);
                if(a<0||b<0)return null;out.write(a*16+b);
            }else out.write(bytes[i]);
        }
        String value=java.nio.charset.StandardCharsets.UTF_8.newDecoder()
                .onMalformedInput(java.nio.charset.CodingErrorAction.REPORT)
                .onUnmappableCharacter(java.nio.charset.CodingErrorAction.REPORT)
                .decode(java.nio.ByteBuffer.wrap(out.toByteArray())).toString();
        for (int index = 0; index < value.length(); index++) if (Character.isISOControl(value.charAt(index))) return null;
        if(value.isEmpty()||".".equals(value)||"..".equals(value)||value.matches(".*[\\p{Cntrl}/\\\\%?#].*"))return null;
        return value;
    }
    private static boolean locale(String value){return value.matches("[A-Za-z]{2}(?:-[A-Za-z]{2})?");}
    private static boolean safeSlug(String value) {
        return !Arrays.asList("event","artist","account","login","payment","checkout","cart","help","recommendation","recommendations","search")
                .contains(value.toLowerCase(Locale.ROOT));
    }

    static boolean challenge(String text) {
        String normalized = text == null ? "" : text.toLowerCase(Locale.ROOT).replaceAll("\\s+", " ");
        String[] markers = { "your browsing activity has been paused", "we’ve paused activity", "we've paused activity", "complete the captcha", "solve the captcha", "captcha verification required",
                "complete the recaptcha", "verify you are human", "verify you're human",
                "verify that you are human", "pardon the interruption", "you are in the queue",
                "you're in the queue", "your place in the queue", "queue is paused",
                "you are now in line", "you're now in line", "waiting room", "access denied",
                "request blocked", "unusual traffic", "unusual activity", "checking your browser",
                "enable cookies to continue", "are you a robot", "temporarily blocked" };
        for (String marker : markers) if (normalized.contains(marker)) return true;
        return false;
    }

    static String classify(boolean identityMatches, boolean verified, boolean validSelectors,
                           boolean available, boolean unavailable, String text) {
        if (challenge(text)) return "BLOCKED";
        if (!identityMatches || !verified || !validSelectors || available == unavailable) return "UNKNOWN";
        return available ? "AVAILABLE" : "UNAVAILABLE";
    }
}
