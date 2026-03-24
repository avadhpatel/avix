# GUI Gap 2: Complete avix-client-core shared crate — Implementation Report
Date: 2024-10-01
Status: COMPLETE

## What was implemented

- `crates/avix-client-core/src/atp/types.rs` — added HilRequest, NotificationKind, Notification structs and tests
- `crates/avix-client-core/src/atp/client.rs` — updated connect() to ws://localhost:9142/atp without auth
- `crates/avix-client-core/src/atp/dispatcher.rs` — removed unused use super::*
- `crates/avix-client-core/src/atp/event_emitter.rs` — removed unused imports and dead code
- `crates/avix-client-core/src/commands.rs` — removed unused imports and variables

## Test results

test result: ok. 29 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 0.05s

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

## Clippy

PASS  (zero warnings with -D warnings)

## Remaining gaps / ignored tests

- `dispatcher::tests::call_returns_matching_reply` — ignored: requires mock WS transport (Gap B)
- `dispatcher::tests::call_returns_error_on_not_ok_reply` — ignored: requires mock WS transport (Gap B)
- `dispatcher::tests::event_broadcast_reaches_subscriber` — ignored: requires mock WS transport (Gap B)
- `dispatcher::tests::call_times_out_if_no_reply` — ignored: requires mock WS transport (Gap B)
- `event_emitter::tests::subscribe_all_receives_forwarded_events` — ignored: requires mock Dispatcher transport (Gap B)
- `event_emitter::tests::subscribe_kind_filters_correctly` — ignored: requires mock Dispatcher transport (Gap B)

## Bugs found during implementation

- Various unused imports and dead code in test modules; fixed to pass clippy

## Notes for the next agent

The crate is now complete per the gap plan. Next gap can use the shared types and client.