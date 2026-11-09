package app.ticketwatch.mobile;

import org.json.JSONArray;
import org.json.JSONObject;
import java.math.BigDecimal;
import java.util.HashSet;
import java.util.Locale;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** Shared regional Ticketmaster layout; never tied to an artist or event identifier. */
public final class GenericTicketParser {
    public static final String ADAPTER = "ticketmaster-regional";
    private GenericTicketParser() { }
    private static String helpers() {
        return "function clean(s){return String(s||'').replace(/\\s+/g,' ').trim().slice(0,240);}"
            + "function visible(e){if(!e||!e.isConnected)return false;for(var p=e;p&&p.nodeType===1;p=p.parentElement){var s=getComputedStyle(p);if(p.hidden||p.getAttribute('aria-hidden')==='true'||s.display==='none'||s.visibility==='hidden'||s.visibility==='collapse'||Number(s.opacity)===0)return false;}var r=e.getBoundingClientRect();return r.width>0&&r.height>0;}"
            + "function disabled(e){for(var p=e;p&&p.nodeType===1;p=p.parentElement)if(p.disabled||p.matches(':disabled')||p.getAttribute('aria-disabled')==='true')return true;return false;}"
            + "function text(e){var w=document.createTreeWalker(e,NodeFilter.SHOW_TEXT),s='',n;while(n=w.nextNode()){var p=n.parentElement;if(p&&!p.closest('button')&&visible(p))s+=' '+n.nodeValue;}return clean(s);}"
            + "function dimensions(row){var out={section:'',row:''},valid=true;var nodes=row.querySelectorAll('dl > div');if(nodes.length>50)return null;Array.prototype.forEach.call(nodes,function(d){var dt=d.querySelector('dt'),dd=d.querySelector('dd');if(!dt||!dd||!visible(dt)||!visible(dd))return;if(d.querySelectorAll('dt').length!==1||d.querySelectorAll('dd').length!==1){valid=false;return;}var key=clean(dt.innerText).toLowerCase(),value=clean(dd.innerText),field=['sektor','section','block','sector','sekce','vak'].indexOf(key)>=0?'section':['rząd','row','reihe','rang','řada','rij'].indexOf(key)>=0?'row':'';if(field){if(out[field])valid=false;out[field]=value;}});return valid?out:null;}"
            + "function blocked(){return /your browsing activity has been paused|we.ve paused activity|access denied|verify (?:that )?you are human|verify you're human|complete the captcha|waiting room|you are (?:now )?in (?:the queue|line)|request blocked|pardon the interruption|unusual traffic|unusual activity|checking your browser|enable cookies to continue|are you a robot|temporarily blocked|queue is paused|your place in the queue|solve the captcha|complete the recaptcha|captcha verification required/i.test((document.body?document.body.innerText:'').slice(0,18000));}";
    }

    /** Only opens the known display tab. It never chooses quantity, listing, or checkout. */
    public static String prepareScript() {
        return "(function(){try{" + helpers()
            + "if(blocked())return {clicked:false,blocked:true};"
            + "var labels=['wybierz najlepsze dostępne miejsca','see best available','beste verfügbare plätze','beste beschikbare plaatsen','mejores entradas disponibles','meilleures places disponibles','nejlepší dostupné','ver lo mejor disponible','best beschikbaar'];"
            + "var candidates=Array.prototype.filter.call(document.querySelectorAll('button,[role=tab]'),function(e){return visible(e)&&!disabled(e)&&labels.indexOf(clean(e.innerText).toLowerCase())>=0;});"
            + "if(candidates.length!==1)return {clicked:false,blocked:false};var e=candidates[0];var maps=['zobacz miejsca na mapie','view seat map','seat map','sitzplan','kaartweergave','mapa míst','bekijk plattegrond','ver mapa de asientos'];if(e.getAttribute('role')!=='tab'&&!Array.prototype.some.call(document.querySelectorAll('button,[role=tab]'),function(b){return visible(b)&&maps.indexOf(clean(b.innerText).toLowerCase())>=0;}))return {clicked:false,blocked:false};"
            + "if(Array.prototype.some.call(document.querySelectorAll('[data-testid=ticketTypeInfo]'),visible)||e.getAttribute('aria-selected')==='true')return {clicked:false,blocked:false};"
            + "e.scrollIntoView({block:'center'});var r=e.getBoundingClientRect(),hit=document.elementFromPoint(r.left+r.width/2,r.top+r.height/2);"
            + "if(!hit||!(hit===e||e.contains(hit)))return {clicked:false,blocked:false};e.click();return {clicked:true,blocked:false};"
            + "}catch(e){return {clicked:false,blocked:false};}})()";
    }

    public static String extractionScript() {
        return "(function(){var out={valid:false,adapter:'ticketmaster-regional',rows:[],categories:[],truncated:false,url:location.href,currencyHints:[],currencyHintsIncomplete:false};try{" + helpers()
            + "if(blocked())return out;var scripts=document.querySelectorAll('script[type=\"application/ld+json\"]'),seen=0;"
            + "function metadata(node,depth){if(!node||typeof node!=='object')return;if(depth>5||++seen>100){out.currencyHintsIncomplete=true;return;}if(Array.isArray(node)){if(node.length>20)out.currencyHintsIncomplete=true;node.slice(0,20).forEach(function(n){metadata(n,depth+1);});return;}"
            + "var types=Array.isArray(node['@type'])?node['@type']:[node['@type']];if(types.some(function(t){return typeof t==='string'&&/^(Event|MusicEvent|SportsEvent|TheaterEvent|DanceEvent|ComedyEvent)$/.test(t.split('/').pop().split('#').pop());})&&typeof node.url==='string'&&node.url.length<=4096){"
            + "var offers=Array.isArray(node.offers)?node.offers:[node.offers];if(offers.length>20)out.currencyHintsIncomplete=true;offers.slice(0,20).forEach(function(o){if(o&&Object.prototype.hasOwnProperty.call(o,'priceCurrency')){if(Object.prototype.hasOwnProperty.call(o,'url')&&(typeof o.url!=='string'||!o.url.trim()))return;if(out.currencyHints.length>=50){out.currencyHintsIncomplete=true;return;}out.currencyHints.push({eventUrl:node.url,offerUrl:typeof o.url==='string'?o.url:'',currency:typeof o.priceCurrency==='string'?o.priceCurrency.slice(0,16):''});}});}"
            + "if(node['@graph'])metadata(node['@graph'],depth+1);}"
            + "if(scripts.length>10)out.currencyHintsIncomplete=true;for(var m=0;m<Math.min(scripts.length,10);m++){var raw=scripts[m].textContent||'';if(raw.length>262144){out.currencyHintsIncomplete=true;continue;}try{metadata(JSON.parse(raw),0);}catch(ignore){out.currencyHintsIncomplete=true;}}"
            + "var infos=document.querySelectorAll('[data-testid=ticketTypeInfo]');if(infos.length>500)return out;"
            + "for(var i=0;i<infos.length;i++){var info=infos[i],li=info.closest('li'),quick=false;if(!li){var candidate=info.closest('div[role=button]');if(candidate&&candidate.parentElement&&candidate.parentElement.matches('[data-testid=quickpicksList]')&&candidate.querySelectorAll('dl').length===1){li=candidate;quick=true;}}if(!li||!visible(info)||disabled(li)||li.querySelectorAll('[data-testid=ticketTypeInfo]').length!==1)continue;"
            + "var spans=Array.prototype.filter.call(info.querySelectorAll('span'),function(e){return !e.closest('button')&&visible(e)&&!Array.prototype.some.call(e.querySelectorAll('span'),function(c){return !c.closest('button');});});"
            + "if(spans.length<2||spans.length>4)continue;var values=spans.map(text),step=li.querySelector('[data-testid=quantityStepper]');"
            + "if((step||quick)&&spans.length===2){var spin=step?step.querySelector('[role=spinbutton]'):null,max=spin&&visible(step)&&visible(spin)&&!disabled(step)&&!disabled(spin)?spin.getAttribute('aria-valuemax'):null;"
            + "if(!values[0]||!/[0-9]/.test(values[1]))continue;var categoryContext=quick?dimensions(li):{section:'',row:''};if(!categoryContext)continue;out.categories.push({type:values[0],price:values[1],section:categoryContext.section,row:categoryContext.row,selectableQuantity:max&&/^[0-9]{1,6}$/.test(max)?Number(max):null});out.valid=true;}"
            + "else if(li.getAttribute('role')==='button'&&spans.length===4&&/^(bilety resale|resale tickets|resale|tickets resale|wiederverkauf|revente|ověřená přeprodávaná vstupenka|verified resale ticket)$/i.test(values[0])&&li.querySelector('dl')){"
            + "var dims=dimensions(li);if(!dims||!values[2]||!/[0-9]/.test(values[3]))continue;out.rows.push({type:values[2],price:values[3],quantity:values[1],section:dims.section,row:dims.row});out.valid=true;}"
            + "if(out.rows.length+out.categories.length>=50){out.truncated=true;break;}}return out;}catch(e){return {valid:false,adapter:'ticketmaster-regional',rows:[],categories:[]};}})()";
    }

    public static TicketData.Snapshot parse(JSONObject payload, boolean identity, String pageState, long at) {
        TicketData.Snapshot result = new TicketData.Snapshot();
        result.detail = "Unsupported or incomplete ticket layout. Ticket availability is unknown.";
        if (!identity || !("UNKNOWN".equals(pageState) || "AVAILABLE".equals(pageState)) || payload == null
                || !payload.optBoolean("valid", false) || !ADAPTER.equals(payload.optString("adapter"))) return result;
        Set<String> seen = new HashSet<>();
        java.util.Map<String,String> categoryPrices = new java.util.HashMap<>();
        String currencyHint = matchedCurrency(payload);
        JSONArray categories = payload.optJSONArray("categories");
        for (int i=0; categories!=null && i<Math.min(categories.length(),50); i++) {
            JSONObject row=categories.optJSONObject(i); if(row==null)continue;
            TicketData.Category c=new TicketData.Category(); c.ticketType=field(row,"type"); c.priceText=field(row,"price");
            c.section=field(row,"section"); c.row=field(row,"row");
            if(c.ticketType.isEmpty()||c.priceText.isEmpty())continue;
            c.currency=currency(c.priceText,currencyHint); String[] bounds=prices(c.priceText,c.currency); c.min=bounds[0];c.max=bounds[1];
            Object selectable=row.opt("selectableQuantity");
            if(selectable instanceof Number)c.selectableQuantity=TicketPageParser.parseQuantity(selectable.toString());
            String categoryKey=c.ticketType+"\n"+c.section+"\n"+c.row;
            String previous=categoryPrices.put(categoryKey,c.priceText);
            if(previous!=null && !previous.equals(c.priceText)) {
                TicketData.Snapshot conflict=new TicketData.Snapshot();
                conflict.detail="Conflicting prices for the same ticket category and location. Availability is unknown.";
                return conflict;
            }
            if(previous==null)result.categories.add(c);
        }
        JSONArray rows=payload.optJSONArray("rows");
        for(int i=0;rows!=null&&i<Math.min(rows.length(),50);i++) {
            JSONObject row=rows.optJSONObject(i);if(row==null)continue;
            TicketData.Offer o=new TicketData.Offer();o.ticketType=field(row,"type");o.section=field(row,"section");o.row=field(row,"row");
            o.priceText=field(row,"price");o.quantityText=field(row,"quantity");o.quantity=listingQuantity(o.quantityText);
            if(o.ticketType.isEmpty()||o.priceText.isEmpty()||o.quantity==null||o.quantity<=0)continue;
            o.quantityKind="listing";o.currency=currency(o.priceText,currencyHint);String[] bounds=prices(o.priceText,o.currency);
            if(bounds[0].equals(bounds[1]))o.price=bounds[0];o.priceScope="per_ticket";
            String key=o.ticketType+"\n"+o.section+"\n"+o.row+"\n"+o.priceText+"\n"+o.quantity;
            if(!seen.add(key))continue;o.key=Models.watchId(key);result.offers.add(o);
        }
        result.verified=true;result.adapter=ADAPTER;result.observedAt=Math.max(0,at);
        result.detail=result.offers.isEmpty()?"Displayed ticket categories. Remaining stock and fulfillment are not confirmed."
                :"Visible resale listings. Counts belong to each listing; total event stock is unknown.";
        if(payload.optBoolean("truncated",false))result.detail="Partial page sample. "+result.detail;
        return result;
    }
    private static Integer listingQuantity(String raw) {
        Matcher m=Pattern.compile("(?iu)^(?:dostępne|dostupných:|available|verfügbar|beschikbaar|disponibles|disponible)\\s+([0-9]{1,6})$").matcher(raw);
        if(m.matches())return TicketPageParser.parseQuantity(m.group(1));
        m=Pattern.compile("(?iu)^([0-9]{1,6})\\s+(?:available|verfügbar|beschikbaar|disponibles|disponible)$").matcher(raw);
        return m.matches()?TicketPageParser.parseQuantity(m.group(1)):null;
    }
    private static String matchedCurrency(JSONObject payload) {
        String current = payload.optString("url", "");
        if (!PageSignals.sameEvent(current,current)) return "";
        if (payload.optBoolean("currencyHintsIncomplete",false)) return "!";
        Set<String> codes=new HashSet<>();
        JSONArray hints=payload.optJSONArray("currencyHints");
        if(hints!=null && hints.length()>50)return "!";
        for(int i=0;hints!=null&&i<Math.min(hints.length(),50);i++) {
            JSONObject hint=hints.optJSONObject(i);if(hint==null)continue;
            String event=hint.optString("eventUrl", ""),offer=hint.optString("offerUrl", "");
            if(!PageSignals.sameEvent(current,event)||(!offer.isEmpty()&&!PageSignals.sameEvent(current,offer)))continue;
            String code=hint.optString("currency", "");
            if(!code.matches("[A-Z]{3}")||"XXX".equals(code)||"XTS".equals(code))return "!";
            try{codes.add(java.util.Currency.getInstance(code).getCurrencyCode());}catch(IllegalArgumentException ignored){return "!";}
        }
        return codes.size()>1?"!":codes.isEmpty()?"":codes.iterator().next();
    }
    static String compatibleCurrency(String text, String expected) { return currency(text, expected); }

    private static String currency(String text,String hint) {
        if ("!".equals(hint)) return "";
        Set<String> codes=new HashSet<>();
        if(!hint.isEmpty())codes.add(hint);
        if(text.contains("Kč"))codes.add("CZK");if(text.contains("zł"))codes.add("PLN");if(text.contains("€"))codes.add("EUR");if(text.contains("£"))codes.add("GBP");
        Matcher m=Pattern.compile("(?<![A-Za-z])[A-Z]{3}(?![A-Za-z])").matcher(text);
        while(m.find())try {String c=java.util.Currency.getInstance(m.group()).getCurrencyCode();if(!"XXX".equals(c)&&!"XTS".equals(c))codes.add(c);}catch(IllegalArgumentException ignored){}
        if(codes.size()!=1)return "";
        String code=codes.iterator().next();
        if(text.contains("$")&&!java.util.Arrays.asList("USD","CAD","AUD","NZD","MXN","SGD","HKD").contains(code))return "";
        if(Pattern.compile("(?iu)(?<![a-z])kr(?![a-z])").matcher(text).find()&&!java.util.Arrays.asList("SEK","NOK","DKK").contains(code))return "";
        if((text.contains("¥")||text.contains("￥"))&&!java.util.Arrays.asList("JPY","CNY").contains(code))return "";
        return code;
    }
    private static String[] prices(String raw,String currency) {
        String[] empty={"",""};if(currency.isEmpty())return empty;
        String value=raw.replace('\u00a0',' ').replace('\u202f',' ').trim();
        String lower=value.toLowerCase(Locale.ROOT);
        if(lower.startsWith("je ")) value=value.substring(3).trim();
        else for(String suffix:new String[]{" za kus"," cada una"," per stuk"," per biljett"," each"}) {
            if(lower.endsWith(suffix)){value=value.substring(0,value.length()-suffix.length()).trim();break;}
        }
        String[] parts=value.split("\\s*[–—]\\s*",-1);if(parts.length>2)return empty;
        String[] out={"",""};
        for(int i=0;i<parts.length;i++) {
            String comma=TicketPageParser.parsePrice(parts[i],",",currency),dot=TicketPageParser.parsePrice(parts[i],".",currency);
            if(!comma.isEmpty()&&!dot.isEmpty()&&!comma.equals(dot))return empty;
            String amount=!comma.isEmpty()?comma:dot;if(amount.isEmpty())return empty;out[i]=amount;
        }
        if(parts.length==1)out[1]=out[0];
        if(new BigDecimal(out[0]).compareTo(new BigDecimal(out[1]))>0)return empty;
        return out;
    }
    private static String field(JSONObject item,String key) {
        Object value=item.opt(key);if(!(value instanceof String))return "";
        String text=((String)value).replaceAll("[\\p{Cntrl}\\s]+"," ").trim();return text.length()>240?text.substring(0,240):text;
    }
}
