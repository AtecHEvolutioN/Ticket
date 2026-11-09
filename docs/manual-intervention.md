# Manual verification extension

The browser code has a manual-intervention boundary in
[`browser/manual-intervention.cjs`](../browser/manual-intervention.cjs). It detects
recognized visible verification, queue, expired-session, authentication and access
block messages. No CAPTCHA solver, third-party solving service, token injection,
or queue bypass is implemented.

## Current behavior

- **Background monitor:** a recognized challenge on an HTTP 2xx page returns
  `manual_intervention` with a fixed reason and `adapter_state = "blocked"`.
  Rust treats this as unknown inventory, including when custom selectors would
  otherwise find ticket buttons. Normal blocked backoff and `Retry-After` handling
  apply. An HTTP 403 with the observed `tm-bl: 1` marker reports the fixed
  `ticketmaster_protection` reason without reading its body. Other error responses
  keep their existing handling. The narrow initial-401 navigation wait described
  in [Czech access](czech-access.md) lets the website finish its own loading flow;
  it does not wait for a human or initiate verification actions.
- **Interactive capture:** after the initial Enter, a recognized challenge pauses
  capture and prompts you to complete verification yourself in the same browser.
  Enter triggers a fresh check of the event URL, response status and visible text.
  Acknowledgement alone cannot clear the pause. Three unsuccessful prompts, an
  unsafe navigation, cancellation, or the overall session timeout stop capture.
- **Notifications:** the monitor's diagnostic/status detail records the fixed
  reason. Challenge-specific Discord/Android notifications and a remote browser
  handoff are not connected yet. Background pages still close after each check;
  only interactive capture retains its page while waiting for you.

Run the interactive capture on a graphical desktop:

```bash
npm run capture -- --url YOUR_PUBLIC_EVENT_URL --output captures/event.html \
  --adapter ticketmaster --timeout-secs 300
```

It opens a fresh session and keeps the existing public-event navigation policy.
Login, cart and checkout navigation are outside this capture tool. It cannot
recover every access block, and successful capture does not establish unattended
access. Detection currently uses known English/Czech visible-text markers, not
every regional prompt or an inventory of challenge widgets.

## Where to connect an operator interface

`waitForManualClearance({ inspect, requestHuman, signal, timeoutMs, maxPrompts })`
is the reusable boundary. `captureSession(options, dependencies)` supplies its
terminal implementation through `dependencies.requestHuman`. A future desktop or
mobile operator interface can supply the same callback.

The callback receives only `{ kind, message, attempt, signal }`. `kind` and
`message` come from a fixed list; page HTML, cookies, challenge content and session
URLs are not passed to it. It should notify the operator, wait for their explicit
acknowledgement, and honor `signal` to cancel pending UI work. Its return value is
ignored; it cannot return a token or a successful-verification flag.

The caller supplies `inspect()`. It must validate the current session, expected
event and response before returning a recognized reason, or `null` when no known
challenge remains. Throwing an error stops the flow. The capture caller validates
before and after reading visible text, and again before publishing its private
capture. `clear` means only that these checks passed, not that tickets are in stock.

Without `requestHuman`, a challenge immediately returns
`{ state: "needs_human", kind }`. With a handler, the default limit is three
prompts and five minutes. Cancellation or timeout rejects the operation and aborts
the handler's signal. Callers remain responsible for closing their browser and
context; the capture tool does so in `finally`.

Do not add a blocking human wait to the current background worker: its Rust caller
serializes browser requests and enforces a short request timeout. A future remote
handoff needs a separate session owner and asynchronous resume protocol so other
watches keep running.

## Future cart integration

Automatic carting is not implemented. A cart flow can use this boundary before
selection and after any verification interruption, but must re-read current offers
and prices after the pause. It must verify the selected quantity, adjacency,
currency and fees, then confirm any actual hold and expiry in that same session.
Cheapest selection must compare complete totals for the requested adjacent group;
a category's advertised minimum price does not establish such a group is available.
Unknown quantities, adjacency or fees cannot prove a matching selection. Payment
remains a separate user action.

## Verification

Unit tests cover bounded prompts, cancellation, timeout, fixed notification fields
and refusal to trust a handler's completion claim. Capture tests check the retained
session and reject remaining challenges, changed events and HTTP errors. Chromium
tests use local fixtures to check both parser modes, cooldown preservation, page
cleanup and hidden/attribution text. Rust protocol tests verify blocked handling
and fixed diagnostics, including unrecognized reason codes.

```bash
cargo test --locked
TICKET_BROWSER_TESTS=1 CHROMIUM_PATH=/usr/bin/chromium npm test
```

These tests do not contact Ticketmaster, solve a real challenge or reserve tickets.
