# Ticket preferences

Android 0.7.0-test defaults to **2 tickets, adjacent seats, cheapest eligible
group**. Open **Settings → Ticket preferences** to change the defaults, or open
an event's ticket preferences to save an override for that event. Clearing the
override makes the event use the current defaults again. An override stores a
complete set of choices, so later default changes do not alter that event.

The editable choices are quantity (1–20, subject to the provider's lower limit),
an optional maximum total including fees, currency, whether seats must be adjacent,
and allowed ticket types, category IDs, sections and rows. Empty filter lists allow
any value. Type, section and row entries are literal labels, matched without case sensitivity;
they are not regular expressions or partial-name searches. Different fields are
combined: a selected group must satisfy every active filter. Within one list,
any matching value is allowed. Category IDs are case-sensitive. Section and row filters apply to every seat.

The maximum is for the **whole group including fees**, not the per-ticket price.
An empty maximum imposes no budget cap. Currency must match the quote; there is
no automatic currency conversion. The current offline planner and native reservation
preparation both support Czech events and CZK only; other saved currencies produce
an explicit unsupported-setting error when applied. Cheapest is the only supported ranking method.
An unchecked adjacency requirement still needs exact, individually identified
seated tickets; standing-ticket selection is not implemented.

The phone saves these choices locally. They are not yet synchronized to an
always-on bot, and saving them does not start a reservation. Use the preferences
export to move them to the Rust planner. Existing page checks and notifications
continue to use their own monitoring settings; saved cart preferences do not
silently change alert filters or polling intervals.

## Export and use with the Rust planner

The Android export and Rust command use the same versioned JSON format, shown in
[the default example](../examples/ticket-preferences.json). Export contains only
ticket preferences and public event URLs, with no API key, Discord token, cookie,
payment data or saved inventory.

```bash
# Create a preferences document with the default quantity of two.
ticket-watch cart-preferences > ticket-preferences.json

# Validate an export, or inspect the effective settings for a selected event.
ticket-watch cart-preferences --file ticket-preferences.json
ticket-watch cart-preferences --file ticket-preferences.json \
  --event-url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657

# Apply settings to supplied quotes; this command remains entirely offline.
ticket-watch cart-plan quotes.json --preferences ticket-preferences.json
```

`cart-plan --preferences` returns both `effective_preferences` and `decision`.
Applying preferences never changes the captured search quantity: a quote for two
tickets cannot be reused as evidence for a newly selected quantity of three.
The request must still carry the expected performance date and matching session,
document, fresh exact-group evidence, price and fee information. Missing type or
category evidence cannot satisfy a corresponding filter. Unknown candidates can
prevent a claim that the selected group is the cheapest.

The planner evaluates supplied claims; JSON imports cannot authenticate live
inventory or authorize a reservation. The native browser submit/receipt adapter
and automatic application of these preferences to live monitoring remain pending.
