# Czech checkout SDK source evidence

The public Czech CheckoutSDK JavaScript describes likely navigation and response
contracts, but **no actual cart receipt was obtained in this source review**. The
[source evidence](evidence/czech-checkout-sdk-source-2026-11-10.json) records the
public URL, downloaded and decoded SHA-256 hashes, exact Czech origins, source
path templates and limitations. The review downloaded only the public asset
configured by the saved event page; it did not invoke SDK methods, reserve tickets
or call cart APIs.

The SDK can navigate to `https://checkout.ticketmaster.cz/{requestId}`. When login
is needed, it constructs `https://identity.ticketmaster.cz/sign-in` with a nested
checkout `redirectUri`. That exact identity route should produce a distinct
`authentication_required` diagnostic and pause for human login. It is not a CAPTCHA result
or proof that a reservation did or did not occur. The source also accepts a
response-supplied checkout URL, so navigation policy must validate that URL against
fixed origins and supported paths rather than trust arbitrary response values.

The event caller can continue into legacy MFOL reservation after the SDK returns.
Its `OK` result supplies a checkout handoff link, not the selected tickets and
complete total. Observe the whole same-attempt browser sequence, including new
documents and frames. A public source route is a capture lead; it does not justify
replaying a request or calling the SDK directly.

The GCO source success shape includes `sessionId`, `expiresAt`, `checkoutUrl` and
`accessTokenValid`. Its provisional shape also includes ticket IDs, quantity,
unit price and seat labels. Those fields do not establish every fee, the actual
performance date, affirmative adjacency or the cheapest available eligible group.
`isExactMatch`, `acceptNonAdjacent=false` and consecutive-looking seat numbers
must not become adjacency evidence. Other source branches use different expiry
units; their fields are not interchangeable Czech receipt contracts.

One fresh normal UI attempt must bind the returned event/date, quantity, actual
seat identities, complete price and fees, native hold reference and expiry to the
durable attempt. Record UTC and monotonic capture times. Keep raw bodies, DOM,
screenshots, session references and checkout URLs private, and use local digests
for shareable correlation. Re-reading a timer must not extend the original hold.
Challenges, login, unsupported routes, errors and partial evidence retain an
uncertain outcome until explicit native evidence resolves it. They must not
trigger another Find tickets action or release the journal lock automatically.

See the [reservation boundary](czech-reservation-flow.md) and
[attempt journal](cart-journal.md) for dispatch and reconciliation requirements.
