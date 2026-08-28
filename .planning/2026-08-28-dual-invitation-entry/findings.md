# Findings

- Current `InvitationCode` is an unconstrained opaque string, but all production issuance uses an 8-character Crockford code displayed as `XXXX-XXXX`.
- Current `IssuedInvitation` carries only code, expiry, and origin; no 256-bit invitation identity or full token exists.
- Cloud and mDNS currently publish only a Sponsor endpoint ticket under the short code.
- Core admission already has a redacted 256-bit `InvitationId` required by JoinRequest.
- Core admission has no unresolved-invitation Joiner state.
- OPAQUE is specified but not yet a dependency; it belongs after invitation resolution is made coherent.
- `PairingInvitationIssuer` currently creates the Core invitation only after Infra returns a short code, then stores it in an Application holder keyed only by that code.
- `IssuePairingInvitationResult` currently exposes only short code, expiry, and availability through Engine and bindings.
- The rendezvous service already stores `sponsorTicket` as opaque text and mDNS already transports an opaque ticket, so both channels can carry a versioned full invitation without a server schema change.
- Existing dependencies already include postcard and URL-safe base64, so the full invitation codec needs no new package.
- The Sponsor holder must index one invitation by both `InvitationId` and short code; current Core `PairingInvitation` has no `InvitationId` field.
- Phase 1 can add and return the full invitation without changing the existing short-code transport, so current pairing behavior remains runnable while the new canonical form is introduced.
- The full invitation can use `postcard` plus URL-safe base64 with an explicit prefix/version; the opaque Sponsor endpoint ticket is already available at issuance.
- End-to-end tamper rejection can rely on the random 256-bit id and Sponsor holder binding: changing id, route, or expiry cannot match the Sponsor's in-memory invitation. Local decoding still rejects malformed/version/size errors.
- Current `InvitationCode` derives Debug and several touched Infra logs print the full short code; the new work must make both invitation forms redacted and remove full-code logging in changed paths.
