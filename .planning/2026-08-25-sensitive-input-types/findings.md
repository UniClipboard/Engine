# Findings

- `InitializeSpaceInput.passphrase` and `passphrase_confirm` are plain `String` values immediately converted to domain `Passphrase` values in `SpaceFacade`.
- `UnlockSpaceInput.passphrase` is a plain `String` immediately converted to domain `Passphrase` in `SpaceFacade`.
- `JoinSpaceInput` has already been corrected to typed `InvitationCode` and domain `Passphrase`.
- Relay credentials already use `RelayAccessToken` inside application-facing inputs; Engine converts its stable secret wrapper at the boundary.
- Config migration uses domain `Passphrase` in the application layer.
- Mobile LAN compatibility values are serialization/compatibility records and are outside this typed application-input issue.
- Selected scope: the three passphrase fields in create/unlock inputs and every constructor/caller affected by those type changes.
