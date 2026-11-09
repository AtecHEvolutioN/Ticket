package app.ticketwatch.mobile;

import org.json.JSONArray;
import org.json.JSONObject;

import java.math.BigDecimal;
import java.util.HashSet;
import java.util.Locale;
import java.util.Set;
import java.util.regex.Pattern;

/** Bounded extraction of visible offer rows. Catalog prices never enter this parser. */
public final class TicketPageParser {
    private static final int MAX_OFFERS = 50;
    private TicketPageParser() { }

    /** An expression evaluated inside the existing event/HTTP/identity guarded page check. */
    public static String extractionScript(TicketData.Profile profile) {
        if (profile == null || !profile.verified || profile.rowSelector.isEmpty() || profile.availableSelector.isEmpty())
            return "({valid:false,truncated:false,rows:[]})";
        return "(function(){var out={valid:true,truncated:false,rows:[]};try{"
                + "var cfg=" + TicketData.profileToJson(profile).toString() + ";"
                + "function visible(e){if(!e||!e.isConnected)return false;for(var p=e;p&&p.nodeType===1;p=p.parentElement){"
                + "var s=getComputedStyle(p);if(p.hidden||p.getAttribute('aria-hidden')==='true'||s.display==='none'||s.visibility==='hidden'||s.visibility==='collapse'||s.contentVisibility==='hidden'||Number(s.opacity)===0)return false;}"
                + "var rs=e.getClientRects();for(var i=0;i<rs.length;i++)if(rs[i].width>0&&rs[i].height>0)return true;return false;}"
                + "function items(row,sel){if(!sel)return [];var a=Array.prototype.slice.call(row.querySelectorAll(sel));if(row.matches(sel))a.unshift(row);return a;}"
                + "function clean(s){return String(s||'').replace(/\\s+/g,' ').trim().slice(0,240);}"
                + "function field(row,sel){var a=items(row,sel),v='',n=0;for(var i=0;i<Math.min(a.length,100);i++){if(!visible(a[i]))continue;var t=clean(a[i].innerText);if(t&&t!==v){v=t;n++;}}return n===1?v:'';}"
                + "function positive(row){var a=items(row,cfg.availableSelector);for(var i=0;i<Math.min(a.length,100);i++){var e=a[i],disabled=e.disabled||e.matches(':disabled');for(var p=e;p&&p!==row.parentElement;p=p.parentElement)if(p.disabled||p.getAttribute('aria-disabled')==='true')disabled=true;if(visible(e)&&!disabled)return true;}return false;}"
                + "function quantity(row){var a=items(row,cfg.quantitySelector);var found=[];for(var i=0;i<Math.min(a.length,100);i++)if(visible(a[i]))found.push(a[i]);if(found.length!==1)return {text:'',selectable:false};"
                + "var e=found[0];if(e.tagName==='SELECT'){if(e.disabled||e.matches(':disabled')||e.getAttribute('aria-disabled')==='true')return {text:'',selectable:true};var max=null;for(var j=0;j<Math.min(e.options.length,1000);j++){var o=e.options[j],group=o.parentElement;var value=String(o.value||'').trim();if(!o.disabled&&!o.hidden&&!(group&&group.tagName==='OPTGROUP'&&(group.disabled||group.hidden))&&/^[0-9]{1,6}$/.test(value)){var n=Number(value);if(n>0&&n<=100000)max=max===null?n:Math.max(max,n);}}return {text:max===null?'':String(max),selectable:true};}return {text:clean(e.innerText),selectable:false};}"
                + "var sels=[cfg.rowSelector,cfg.availableSelector,cfg.typeSelector,cfg.sectionSelector,cfg.priceSelector,cfg.quantitySelector];for(var s=0;s<sels.length;s++)if(sels[s])document.querySelector(sels[s]);"
                + "var rows=document.querySelectorAll(cfg.rowSelector);if(rows.length>500)out.truncated=true;"
                + "for(var r=0;r<Math.min(rows.length,500);r++){var row=rows[r];if(!visible(row)||!positive(row))continue;if(out.rows.length>=50){out.truncated=true;break;}var q=quantity(row);"
                + "out.rows.push({id:clean(row.getAttribute('data-offer-id')||row.getAttribute('data-ticket-id')||row.id),type:field(row,cfg.typeSelector),section:field(row,cfg.sectionSelector),price:field(row,cfg.priceSelector),quantity:q.text,quantitySelectable:q.selectable});}"
                + "return out;}catch(e){return {valid:false,truncated:false,rows:[]};}})()";
    }

    /** A visible row is useful only after its host/event and page state have been validated. */
    public static TicketData.Snapshot parse(JSONObject payload, TicketData.Profile profile,
                                            boolean identity, String pageState, long observedAt) {
        TicketData.Snapshot result = new TicketData.Snapshot();
        result.detail = "Offer details are not verified for this page.";
        if (!identity || !"AVAILABLE".equals(pageState) || profile == null || !profile.verified
                || profile.rowSelector.isEmpty() || profile.availableSelector.isEmpty()
                || payload == null || !payload.optBoolean("valid", false)) return result;
        JSONArray rows = payload.optJSONArray("rows");
        if (rows == null) return result;
        Set<String> seen = new HashSet<>();
        for (int i = 0; i < Math.min(rows.length(), MAX_OFFERS); i++) {
            JSONObject row = rows.optJSONObject(i);
            if (row == null) continue;
            TicketData.Offer offer = new TicketData.Offer();
            offer.ticketType = field(row, "type");
            offer.section = field(row, "section");
            offer.priceText = field(row, "price");
            offer.currency = currency(profile.currency);
            if (!offer.currency.isEmpty()) offer.currency = GenericTicketParser.compatibleCurrency(offer.priceText, offer.currency);
            offer.price = parsePrice(offer.priceText, profile.decimalSeparator, offer.currency);
            offer.priceScope = oneOf(profile.priceScope, "per_ticket", "total");
            offer.fees = oneOf(profile.fees, "included", "excluded");
            offer.quantityText = field(row, "quantity");
            offer.quantity = parseQuantity(offer.quantityText);
            offer.quantityKind = row.optBoolean("quantitySelectable", false) ? "selectable"
                    : oneOf(profile.quantityKind, "listing", "selectable", "remaining");
            if (offer.quantity == null) offer.quantityKind = "unknown";
            // Zero capacity is not an available offer, even when another row signal matches.
            if (offer.quantity != null && offer.quantity == 0) continue;
            if (offer.ticketType.isEmpty() && offer.section.isEmpty() && offer.priceText.isEmpty() && offer.quantityText.isEmpty()) continue;
            // Identical visible rows cannot inflate displayed counts. Prices/quantities are not
            // summed because different ticket types may refer to overlapping seat inventory.
            String identityText = offer.ticketType + "\n" + offer.section + "\n" + offer.priceText
                    + "\n" + offer.currency + "\n" + offer.quantityText + "\n" + offer.quantityKind;
            if (!seen.add(identityText)) continue;
            String sourceId = field(row, "id");
            offer.key = Models.watchId(sourceId.isEmpty() ? offer.ticketType + "\n" + offer.section : "source\n" + sourceId);
            result.offers.add(offer);
        }
        result.verified = true;
        result.observedAt = Math.max(0, observedAt);
        result.source = "web";
        result.detail = payload.optBoolean("truncated", false) || rows.length() > MAX_OFFERS
                ? "Partial page sample (limit reached). Counts apply to individual offers, not total event stock."
                : "Visible offers only. Counts apply to individual offers; total event stock is not known.";
        return result;
    }

    /** Strict decimal money parsing; unrecognized or ambiguous text remains display-only. */
    public static String parsePrice(String raw, String decimalSeparator, String currency) {
        String code = currency(currency);
        if (raw == null || raw.length() > 240 || code.isEmpty()
                || !(".".equals(decimalSeparator) || ",".equals(decimalSeparator))) return "";
        String value = raw.trim().replace('\u00a0', ' ').replace('\u202f', ' ');
        String unit = Pattern.quote(code);
        String symbol = "";
        if ("PLN".equals(code)) symbol = "|zł";
        else if ("CZK".equals(code)) symbol = "|Kč";
        else if ("SEK".equals(code) || "NOK".equals(code) || "DKK".equals(code)) symbol = "|kr";
        else if ("EUR".equals(code)) symbol = "|€";
        else if ("GBP".equals(code)) symbol = "|£";
        else if ("USD".equals(code) || "CAD".equals(code) || "AUD".equals(code) || "NZD".equals(code)) symbol = "|\\$";
        String marker = "(?i:(?:" + unit + symbol + "))";
        value = value.replaceFirst("^" + marker + "\\s*", "").replaceFirst("\\s*" + marker + "$", "").trim();
        String decimal = Pattern.quote(decimalSeparator);
        String group = ",".equals(decimalSeparator) ? "\\." : ",";
        String integer = "(?:[0-9]+|[0-9]{1,3}(?:" + group + "[0-9]{3})+|[0-9]{1,3}(?: [0-9]{3})+)";
        int precision = java.util.Currency.getInstance(code).getDefaultFractionDigits();
        String fraction = precision > 0 ? "(?:" + decimal + "[0-9]{1," + precision + "})?" : "";
        if (!value.matches(integer + fraction)) return "";
        value = value.replace(" ", "").replace(",".equals(decimalSeparator) ? "." : ",", "");
        if (",".equals(decimalSeparator)) value = value.replace(',', '.');
        try {
            BigDecimal number = new BigDecimal(value);
            if (number.signum() < 0 || number.compareTo(new BigDecimal("1000000000")) > 0) return "";
            return number.stripTrailingZeros().toPlainString();
        } catch (NumberFormatException ignored) { return ""; }
    }

    public static Integer parseQuantity(String value) {
        if (value == null || !value.matches("[0-9]{1,6}")) return null;
        try { int count = Integer.parseInt(value); return count <= 100000 ? count : null; }
        catch (NumberFormatException ignored) { return null; }
    }

    private static String currency(String raw) {
        String value = Models.safe(raw).trim().toUpperCase(Locale.ROOT);
        if (!value.matches("[A-Z]{3}") || "XXX".equals(value) || "XTS".equals(value)) return "";
        try { java.util.Currency.getInstance(value); return value; }
        catch (IllegalArgumentException ignored) { return ""; }
    }

    private static String field(JSONObject object, String key) {
        Object raw = object.opt(key);
        if (!(raw instanceof String)) return "";
        String value = ((String) raw).replaceAll("[\\p{Cntrl}\\s]+", " ").trim();
        return value.length() > 240 ? value.substring(0, 240) : value;
    }

    private static String oneOf(String value, String... allowed) {
        for (String candidate : allowed) if (candidate.equals(value)) return value;
        return "unknown";
    }
}
