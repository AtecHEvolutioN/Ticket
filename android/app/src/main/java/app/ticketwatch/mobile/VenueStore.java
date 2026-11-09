package app.ticketwatch.mobile;

import android.content.Context;
import android.content.SharedPreferences;
import org.json.JSONArray;
import org.json.JSONObject;
import java.net.URI;
import java.text.Normalizer;
import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.UUID;

/** Private, static venue references. No inventory, prices, page profiles or sessions are stored here. */
public final class VenueStore {
    private static final Object LOCK = new Object();
    private static final int MAX_BYTES = 4 * 1024 * 1024;
    private final SharedPreferences preferences;

    public static final class Venue {
        public String key="", site="", venueId="", name="", city="", country="";
    }
    public static final class Preset {
        public String id="", venueKey="", name="", notes="";
        public List<String> sectionReferences=new ArrayList<>();
    }
    private static final class Data {
        final List<Venue> venues=new ArrayList<>();
        final List<Preset> presets=new ArrayList<>();
    }
    public VenueStore(Context context) {
        preferences=context.getApplicationContext().getSharedPreferences("venue_references",Context.MODE_PRIVATE);
    }

    public static String keyFor(Models.Watch watch) {
        if(watch==null || watch.demo)return "";
        String url=DiscoveryClient.normalizePublicUrl(watch.url);
        if(!PageSignals.sameEvent(url,url))return "";
        try {
            String site=new URI(url).getHost().toLowerCase(Locale.ROOT);
            if(site.startsWith("www."))site=site.substring(4);
            return key(site,clean(watch.venueId),clean(watch.venue),clean(watch.city),country(watch.country));
        }catch(Exception ignored){return "";}
    }
    private static String key(String site,String id,String name,String city,String country) {
        if(!DiscoveryClient.isTicketmasterSite(site)||id.length()>150||name.length()>500||city.length()>200
                ||(!country.isEmpty()&&!country.matches("[A-Z]{2}")))return "";
        if(id.isEmpty()&&(name.isEmpty()||city.isEmpty()||country.isEmpty()))return "";
        JSONArray identity=new JSONArray();identity.put(site);identity.put(id.isEmpty()?"exact_location":"provider_id");
        if(id.isEmpty()){identity.put(label(name));identity.put(label(city));identity.put(country);}else identity.put(id);
        return "venue-"+Models.watchId(identity.toString()).substring(6);
    }
    private static Venue from(Models.Watch watch) {
        String key=keyFor(watch);if(key.isEmpty())return null;
        try {
            Venue v=new Venue();v.key=key;v.site=new URI(watch.url).getHost().toLowerCase(Locale.ROOT);
            if(v.site.startsWith("www."))v.site=v.site.substring(4);
            v.venueId=clean(watch.venueId);v.name=clean(watch.venue);v.city=clean(watch.city);v.country=country(watch.country);return v;
        }catch(Exception ignored){return null;}
    }
    private static boolean compatible(Venue first,Venue second) {
        return first!=null&&second!=null&&first.site.equals(second.site)
                && !conflict(first.city,second.city)&&!conflict(first.country,second.country)
                && (first.venueId.isEmpty()||second.venueId.isEmpty()||first.venueId.equals(second.venueId));
    }
    private static boolean conflict(String a,String b){return !a.isEmpty()&&!b.isEmpty()&&!label(a).equals(label(b));}
    private static boolean exactLocation(Venue a,Venue b){
        return a.site.equals(b.site)&&!a.name.isEmpty()&&!a.city.isEmpty()&&!a.country.isEmpty()
                && label(a.name).equals(label(b.name))&&label(a.city).equals(label(b.city))&&a.country.equals(b.country);
    }
    private static Venue find(Data data,String key){for(Venue v:data.venues)if(v.key.equals(key))return v;return null;}
    /** Provider IDs are authoritative; a name-only observation must have exactly one candidate. */
    private static Venue resolve(Data data,Venue candidate) {
        Venue match=null;
        for(Venue venue:data.venues){
            boolean same=candidate.venueId.isEmpty()?exactLocation(venue,candidate)
                    :venue.site.equals(candidate.site)&&venue.venueId.equals(candidate.venueId);
            if(same){if(match!=null)return null;match=venue;}
        }
        return compatible(match,candidate)?match:null;
    }

    public Venue register(Models.Watch watch) {
        Venue candidate=from(watch);if(candidate==null)return null;
        synchronized(LOCK) {
            Data data=read();Venue current=resolve(data,candidate);boolean changed=false;
            if(current==null){
                // An already-known ID with a conflicting location is never replaced.
                if(!candidate.venueId.isEmpty())for(Venue known:data.venues)
                    if(known.site.equals(candidate.site)&&known.venueId.equals(candidate.venueId))return null;
                List<Venue> exact=new ArrayList<>();
                for(Venue known:data.venues)if(exactLocation(known,candidate))exact.add(known);
                if(candidate.venueId.isEmpty()&&!exact.isEmpty())return null; // Multiple IDs: ambiguous.
                if(!candidate.venueId.isEmpty()&&exact.size()==1&&exact.get(0).venueId.isEmpty()){
                    // Preserve the stable local key and every preset reference during promotion.
                    current=exact.get(0);current.venueId=candidate.venueId;changed=true;
                }else{
                    if(find(data,candidate.key)!=null)return null; // A promoted/renamed venue owns this stable key.
                    if(data.venues.size()>=1000)throw new IllegalStateException("The venue library is full.");
                    data.venues.add(candidate);current=candidate;changed=true;
                }
            }
            if(!candidate.name.isEmpty()&&!candidate.name.equals(current.name)){current.name=candidate.name;changed=true;}
            if(current.city.isEmpty()&&!candidate.city.isEmpty()){current.city=candidate.city;changed=true;}
            if(current.country.isEmpty()&&!candidate.country.isEmpty()){current.country=candidate.country;changed=true;}
            if(changed)write(data);return current;
        }
    }
    public Venue forWatch(Models.Watch watch) {
        Venue candidate=from(watch);if(candidate==null)return null;
        synchronized(LOCK){return resolve(read(),candidate);}
    }
    public List<Venue> all(){synchronized(LOCK){return read().venues;}}
    public List<Preset> presetsFor(Models.Watch watch) {
        Venue venue=forWatch(watch);return venue==null?new ArrayList<>():presetsForVenue(venue.key);
    }
    public List<Preset> presetsForVenue(String venueKey) {
        synchronized(LOCK){Data data=read();List<Preset> result=new ArrayList<>();if(find(data,venueKey)==null)return result;
            for(Preset preset:data.presets)if(preset.venueKey.equals(venueKey))result.add(preset);return result;}
    }
    public Preset savePreset(Models.Watch watch,String name,List<String> sections,String notes) {
        String title=clean(name),memo=Models.safe(notes).trim();
        if(title.isEmpty()||title.length()>80||memo.length()>1000)throw new IllegalArgumentException("Use a preset name up to 80 characters and notes up to 1,000 characters.");
        LinkedHashSet<String> refs=new LinkedHashSet<>();
        if(sections!=null)for(String section:sections){String ref=clean(section);if(ref.isEmpty())continue;
            if(ref.length()>80||refs.size()>=50)throw new IllegalArgumentException("Use up to 50 section references, each at most 80 characters.");refs.add(ref);}
        Venue candidate=register(watch);if(candidate==null)throw new IllegalArgumentException("This event has no unambiguous saved venue.");
        synchronized(LOCK){Data data=read();Venue current=resolve(data,from(watch));if(current==null||!current.key.equals(candidate.key))throw new IllegalArgumentException("The venue identity changed.");
            if(data.presets.size()>=2000)throw new IllegalStateException("The venue preset library is full.");
            Preset preset=new Preset();preset.id="preset-"+UUID.randomUUID();preset.venueKey=current.key;preset.name=title;preset.notes=memo;preset.sectionReferences.addAll(refs);
            data.presets.add(preset);write(data);return preset;}
    }
    /** No watch data is copied: this calls the watch store's atomic reference-only update. */
    public boolean apply(WatchStore watches,String watchId,String presetId){return watches.setVenuePreset(watchId,presetId);}
    public Preset selected(Models.Watch watch) {
        if(watch==null||Models.safe(watch.venuePresetId).isEmpty())return null;
        for(Preset preset:presetsFor(watch))if(preset.id.equals(watch.venuePresetId))return preset;return null;
    }
    public void removePreset(String id){synchronized(LOCK){Data data=read();data.presets.removeIf(p->p.id.equals(id));write(data);}}
    public void removeVenue(String key){synchronized(LOCK){Data data=read();data.venues.removeIf(v->v.key.equals(key));data.presets.removeIf(p->p.venueKey.equals(key));write(data);}}

    private Data read() {
        Data data=new Data();
        try {
            String raw=preferences.getString("registry","");if(raw==null||raw.isEmpty()||raw.length()>MAX_BYTES)return data;
            JSONObject root=new JSONObject(raw);if(root.optInt("schema",0)!=1)return data;
            JSONArray venues=root.optJSONArray("venues");
            for(int i=0;venues!=null&&i<Math.min(venues.length(),1000);i++){
                JSONObject o=venues.optJSONObject(i);if(o==null)continue;Venue v=new Venue();
                v.site=field(o,"site",100);v.venueId=field(o,"venueId",150);v.name=field(o,"name",500);v.city=field(o,"city",200);v.country=country(field(o,"country",2));
                v.key=field(o,"key",100);
                // The local key remains stable after a fallback gains an official provider ID.
                if(v.key.matches("venue-[a-f0-9]{64}")&&!key(v.site,v.venueId,v.name,v.city,v.country).isEmpty()
                        &&find(data,v.key)==null)data.venues.add(v);
            }
            JSONArray presets=root.optJSONArray("presets");LinkedHashSet<String> ids=new LinkedHashSet<>();
            for(int i=0;presets!=null&&i<Math.min(presets.length(),2000);i++){
                JSONObject o=presets.optJSONObject(i);if(o==null)continue;Preset p=new Preset();p.id=field(o,"id",80);p.venueKey=field(o,"venueKey",100);p.name=field(o,"name",80);p.notes=field(o,"notes",1000);
                if(!p.id.matches("preset-[a-f0-9-]{36}")||p.name.isEmpty()||find(data,p.venueKey)==null||!ids.add(p.id))continue;
                JSONArray refs=o.optJSONArray("sectionReferences");LinkedHashSet<String> unique=new LinkedHashSet<>();
                for(int j=0;refs!=null&&j<Math.min(refs.length(),50);j++){Object value=refs.opt(j);if(value instanceof String){String ref=clean((String)value);if(!ref.isEmpty()&&ref.length()<=80)unique.add(ref);}}
                p.sectionReferences.addAll(unique);data.presets.add(p);
            }
        }catch(Exception ignored){return new Data();}return data;
    }
    private void write(Data data) {
        try {
            JSONObject root=new JSONObject();root.put("schema",1);JSONArray venues=new JSONArray(),presets=new JSONArray();
            for(Venue v:data.venues){JSONObject o=new JSONObject();o.put("key",v.key);o.put("site",v.site);o.put("venueId",v.venueId);o.put("name",v.name);o.put("city",v.city);o.put("country",v.country);venues.put(o);}
            for(Preset p:data.presets){JSONObject o=new JSONObject();o.put("id",p.id);o.put("venueKey",p.venueKey);o.put("name",p.name);o.put("notes",p.notes);o.put("sectionReferences",new JSONArray(p.sectionReferences));presets.put(o);}
            root.put("venues",venues);root.put("presets",presets);String raw=root.toString();
            if(raw.getBytes(java.nio.charset.StandardCharsets.UTF_8).length>MAX_BYTES||!preferences.edit().putString("registry",raw).commit())throw new IllegalStateException();
        }catch(Exception ignored){throw new IllegalStateException("The venue references could not be saved.");}
    }
    private static String field(JSONObject o,String key,int maximum){Object value=o.opt(key);if(!(value instanceof String))return "";String s=clean((String)value);return s.length()<=maximum?s:"";}
    private static String label(String raw){return clean(raw).toLowerCase(Locale.ROOT);}
    private static String country(String raw){return clean(raw).toUpperCase(Locale.ROOT);}
    private static String clean(String raw){return Normalizer.normalize(Models.safe(raw),Normalizer.Form.NFC).trim().replaceAll("[\\p{Cntrl}\\s]+"," ");}
}
