# Czech native reservation boundary

**Find tickets can initiate a reservation.** It must not be used as a read-only
availability probe. The reviewed public frontend source connects
`button[data-testid=findTicketsBtn]` to the reservation action, SDK checkout and
platform-specific reservation processing. Commit the durable attempt and dispatch
marker **before** activating this control.

The [reviewed evidence](evidence/czech-reservation-flow-2026-11-10.json) records
public asset URLs and SHA-256 hashes. This review used saved event captures and
downloaded public JavaScript. It made no reservation, private API or purchase
request, and obtained no actual checkout receipt.

## Observed source sequence

1. The button checks the selected quantity/type and dispatches module `45064`,
   export `Pd`. Information, non-adjacent-seat or accessibility dialogs can stop
   the flow before its `RESERVE_START` action.
2. The action enters SDK or platform-specific processing. The saved Czech event
   used `MFOL`, with `checkoutSdk=true` and `checkoutSdkHighDemand=false`.
   The source can call `CheckoutSDK.checkout`; its legacy MFOL fallback submits a
   reservation request and follows a returned checkout link. These captured flags
   are historical, so the adapter must observe the active flow again.
3. Reservation processing can also return non-adjacent/block-size errors and open
   the non-adjacent acceptance dialog. The requested adjacent-seat policy requires
   stopping there. An absent warning, `hasAcceptedNonAdjacent=false`, exact-match
   flags or consecutive-looking seat numbers do not prove adjacency.
4. Some supported response branches populate reservation details and start a
   timer. The bundle includes `reserveView` and translated timer rendering, but
   their presence in source does not establish the current Czech checkout layout
   or a hold in the saved page.

A timeout or navigation after the click is an uncertain outcome. Reconcile the
same session before considering another attempt. A return to selection does not
by itself prove that previously held tickets were released. The
[attempt journal](cart-journal.md) records this boundary. The experimental
[native cart command](native-cart-attempt.md) now connects the dispatch boundary
to one ordinary UI action; it does not yet authenticate a hold receipt.

## Evidence still required for the adapter

The saved page has no reservation details and an empty basket. Earlier offline
form research produced `Vstupenek v košíku: 1` after changing a demo quantity with
networking disabled. That counter is selection state, not hold evidence.
Likewise, `Cena zahrnuje servisní poplatek` does not establish every checkout fee.

Before accepting a native receipt, obtain a fresh sample and verify its event and
event date, exact user-requested quantity, seat identities, affirmative adjacency,
complete group total in CZK, and actual hold reference/expiry in the same session.
The source contains several response normalizers with different price and expiry
fields; none is yet a validated Czech receipt contract. Its alternate-date result
branch reinforces the need to bind the resulting event date separately.

Saved [ticket preferences](ticket-preferences.md) now default to two adjacent
tickets and can be changed globally or per event. Category minima overlap, and
the cheapest configured category or first successful reservation cannot prove the
cheapest available adjacent group. Exhaustive search coverage also remains
unverified; do not create multiple holds to manufacture that evidence.

The [native controls inspector](cart-controls.md) now implements the non-reserving
quantity/filter step: bind a readable event document, disable networking before a
demo form change, inspect the labeled checkboxes in `panelPriceSection`, close it
with `panelClose`/`panelBack` and restore zero selection. Escape did not close the
earlier saved panel. Its result remains configuration only.

The [journal's native-search workflow](cart-journal.md#a-native-search-that-may-reserve)
can now persist the requested quantity and filters before Find tickets without
pretending that the later quote already existed. Actual submission now runs in a
separate retained browser. Native receipt extraction remains incomplete, so its
observations retain uncertainty. Durable reconciliation of supplied native outcome
evidence is available; accepting a confirmed hold still requires a validated
receipt contract.

The [native preparation rehearsal](native-preparation.md) applies an explicit
configured price and compatible native area filters with the saved quantity,
verifies their mapping, and restores them offline. It does not use the lowest
configured price as proof of the cheapest available adjacent group.
