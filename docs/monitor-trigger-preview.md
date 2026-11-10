# Saved preferences and monitor trigger preview

The server can now save a versioned ticket-preferences document and compare it
with a watch's latest stored observation. This is the first read-only connection
between monitoring and cart preferences. It never opens Ticketmaster, changes a
quantity, reserves tickets, sends Discord messages or updates a cart journal.

## Save and review preferences

Use an Android export, or create the existing default document:

```bash
ticket-watch cart-preferences > ticket-preferences.json
ticket-watch cart-preferences-save ticket-preferences.json \
  --store data/ticket-preferences.sqlite --expected-revision 0
```

Revision zero means the registry must not exist yet. The first successful save
returns revision one, a document hash and an update timestamp. The registry is a
separate database; do not point it at the monitor or cart journal database.

Inspect the registry or resolve an event override:

```bash
ticket-watch cart-preferences --store data/ticket-preferences.sqlite
ticket-watch cart-preferences --store data/ticket-preferences.sqlite \
  --event-url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657
```

After editing the export, save it with the revision you last inspected:

```bash
ticket-watch cart-preferences-save ticket-preferences.json \
  --store data/ticket-preferences.sqlite --expected-revision 1
```

A changed document advances the revision. A stale expected revision fails instead
of overwriting someone else's changes. Saving the identical document against its
current revision keeps the revision and timestamp. The hash represents the parsed
document, including list order; it is not a proof of equivalent preference meaning
or a provider signature.

Export retains the schema used by Android and the existing cart commands:

```bash
ticket-watch cart-preferences --store data/ticket-preferences.sqlite \
  --export --expected-revision 2 > reviewed-preferences.json
ticket-watch cart-plan quotes.json --preferences reviewed-preferences.json
```

Use the actual current revision in these examples. Inspection/export never creates
a missing registry. Imports validate the complete document before creating it;
unrelated or empty existing files are refused. This local registry does not sync
with Android or Discord, and existing live cart commands still consume explicit
preference files. Saving a revision changes neither alert filters nor active
reservation requests.

## Preview a selected watch

Use the same monitor configuration and an existing manual or generated watch ID:

```bash
ticket-watch --config config.local.toml cart-trigger-preview \
  --watch WATCH_ID --preferences-store data/ticket-preferences.sqlite \
  --expected-revision 2
```

Preparation previews currently support Czech Ticketmaster events and CZK.
The registry can retain other saved currencies and event overrides; unsupported
preview settings return an explicit reason without recommending preparation.

The command reads the existing monitor database in a read-only snapshot. It binds
the saved observation to the current watch configuration, including its event
identity; reusing a watch ID for another event cannot reuse old inventory.
Generated watches must still belong to the current cached subscription/profile
configuration. A blocked latest observation stays blocked even if an older
positive baseline remains in the monitor.

Only a fresh positive offer observation with sufficient observed listing or
reported remaining quantity and no known preference mismatch can recommend a
fresh native preparation. Metadata availability, displayed category ranges and
selectable order capacity cannot establish that signal. Observations older than
five seconds, future timestamps and saved-capture timestamps of zero do not
qualify. Monitor timestamps have one-second resolution; freshness uses their
stored UTC time conservatively and is not a measurement of inventory-release time.
The preview does not refresh stale data; it needs the next normal monitor check.

The result identifies the preference revision and resolved settings, gives fixed
reasons and bounded per-offer checks, and always reports
`reservation_authorized: false`. Missing evidence remains unknown. Diagnostics
do not publish raw offer keys, page details or captured HTML.

The budget check concerns displayed amounts. A known per-ticket subtotal above
the group cap is a mismatch even when fees are unknown. A subtotal below the cap
with missing fees does not prove the complete total fits. A listing total cannot
be resized for a different requested quantity. Even a displayed amount marked as
including fees does not authenticate the final checkout total.

`needs_fresh_native_preparation: true` is a recommendation to obtain new evidence,
not a cart plan or a verified adjacent group. Performance identity, exact seats,
adjacency, complete checkout fees, cheapest-group coverage and existing cart locks
still need their respective checks. The preview neither queues a trigger nor
submits one. A later preview must reread current preferences and source data;
an earlier result does not authorize execution after a revision or source change.

These source commands are newer than the 0.7.0-test downloads. The APK is unchanged.
