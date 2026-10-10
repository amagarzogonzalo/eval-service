# Conformance fixture set (M1 — definitions, enforced in M3/M5 runner tests)
#
# Every target (including both reference shims from M3) must survive:
# echo (happy path), hang (-> Timeout), crash (-> Crash),
# malformed output (-> ProtocolError), wrong-version handshake
# (-> clean reject), unknown-field case (tolerated by runner,
# rejected by strict target parse).
#
# Files:
# - echo_case.json: runner sends, target must answer echo_result.json shape
# - hang_case.json: target never answers within limits.timeout_ms
# - crash_case.json: target exits non-zero before a terminal message
# - malformed_stdout.txt: raw stdout line -> ProtocolError
# - wrong_version_handshake.json: -> VersionMismatch
# - unknown_field_case.json: runner tolerates, strict target rejects
