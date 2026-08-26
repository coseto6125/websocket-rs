# Follow-ups

Deferred items with a stub here so they stay auditable. Each entry flips to
DONE (with the closing PR) once merged.

- [ ] F8: `build_handshake` splices user headers and subprotocols into raw
  request bytes without token/CRLF validation — `subprotocols=["a\r\nX-Evil: y"]`
  injects handshake headers on the native client. Pre-existing on main; the
  sync path validates via `HeaderValue::from_str` since 0.7.8. Fix = shared
  token validation before splicing.
  Source: PR #45 correctness review (confidence 15, not a regression).
