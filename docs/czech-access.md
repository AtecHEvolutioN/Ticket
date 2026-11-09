# Czech event access: diagnosis and browser navigation

Checked on 2026-11-09 against
[KoRn at O2 arena, Prague](https://www.ticketmaster.cz/event/korn-vstupenky/1472650657).
The [sanitized observation](evidence/czech-access-2026-11-09.json) records the
capture time, response sequence and displayed prices. Raw HTML, cookies, session
identifiers and protection payloads remain private.

## What caused the reported error

The headless browser received HTTP 403 with `tm-bl: 1`. Its response included a
Ticketmaster identity-verification fallback saying “Let's Get Your Identity
Verified” and “we need to make sure you're not a bot.” Ticketmaster's protection
scripts and block-page resources loaded successfully. Cookies were created and
JavaScript was enabled. The fallback text was inside `noscript`, so the normal
document title/body text inspection did not reveal it.

This identifies an access-protection response, but not Ticketmaster's private risk
rule. `server: envoy` alone does not establish its origin. A plain HTTP client
separately received HTTP 401 with `{"response":"identify"}`; a Discovery API key
does not authenticate these website requests.

A fresh **headed Chromium** session, using the same configured Cloud proxy,
received an initial HTTP 401 HTML identification page with `tm-bl: 1`. The
website's own JavaScript then navigated to the same event with HTTP 200. The event
title, date, venue and ticket-selection display loaded. No browser identity was
spoofed, session imported, CAPTCHA solved, or ticket selected by our code.

This proves that this browser configuration could reach the event in the tested
session. It does not isolate a particular fingerprint check, establish an identical
exit IP for both requests, or guarantee continued access.

The optimized release CLI was subsequently checked through the actual worker in
headed mode. It returned HTTP 200 with `adapter = ticketmaster-regional-v1`, the
same four price categories, and unknown inventory. Local verification passed
167 Rust tests, 45 browser tests including real Chromium fixtures, Clippy and
format checks. The independent lifecycle review found no remaining blocking
issues after the recorded regression cases were corrected.

## Worker correction

The worker previously treated the first navigation response as final and closed
the page immediately on an HTTP error. That ended the headed session before the
website's normal identification navigation completed.

The corrected worker allows a passive wait of at most five seconds, within the
request deadline, for the observed initial **401 + `tm-bl: 1` + HTML** response on
a validated event. A supplied `Retry-After` must parse as zero; positive, invalid
or combined values prevent this wait. It initiates no reload, retry or verification
action. It accepts inventory markup only from a
committed, loaded successful document for the same event, and checks that the
document remains current through extraction.

If follow-through times out, aborts or never commits, the worker retains the
initial observed 401 with empty HTML and the fixed `identification_incomplete`
reason. Its URL identifies that reported error response. This preserves the
blocked cooldown without treating an unfinished page as a successful final
document. Unsafe redirects still fail the public-event guard.

An initial 403, 429 or unrelated 401 retains its normal error handling. A 403 with
the exact observed `tm-bl: 1` marker receives the fixed diagnostic
“Ticketmaster access-protection block; exact trigger undisclosed.” Error bodies
are not extracted as inventory. The direct HTTP transport reports the same narrow
403 diagnosis without downloading the error body.

## Run on a graphical desktop

Build the current source and install the browser dependencies as described in
[the runbook](running.md), then run:

```bash
./target/release/ticket-watch inspect --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657
```

For an installed Chromium, set `CHROMIUM_PATH` to its executable. In the monitor
configuration, use `[browser] headed = true` and a browser-mode watch. A graphical
display must be available to the process. Headless mode remains the default;
the application does not automatically switch browser modes after a block.

The Cloud comparison used a temporary Xvfb display. This provides a graphical
browser test, not a working remote desktop handoff. The shared desktop control
tool was unavailable in this session. Manual intervention still requires the
operator workflow described in [manual verification](manual-intervention.md).

## Price snapshot

Opening the recognized Czech best-available display tab exposed these categories:

| Displayed category | Price per ticket |
| --- | --- |
| Restricted view — Omezený výhled | 1,712–2,125 CZK |
| Seating — Sezení | 1,889–2,361 CZK |
| Standing — Stání | 2,125 CZK |
| Official Platinum | 2,590–5,591 CZK |

The page says the service fee is included. Complete checkout fees were not
verified. The displayed event purchase limit was six; that is not a count of
remaining tickets. No resale listings were observed in this capture. These
category ranges do not confirm an available adjacent group, and the parser
correctly leaves inventory availability unknown. Automatic carting is not
implemented; continuous production monitoring remains unverified.
