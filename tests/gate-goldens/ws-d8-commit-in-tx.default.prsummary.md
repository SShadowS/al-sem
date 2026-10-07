### ⛔ Transaction integrity — 1 high, 1 info

**HIGH**  [d35-commit-in-event-subscriber] Commit reachable from event subscriber
  App: PT/D8 Tx Span 1.0.0.0  —  "D8BadSubscriber".HandlePosted()
  ws:src/subscriber.al:4  [EventSubscriber] HandlePosted
  ws:src/subscriber.al:11  Commit
  coverage: complete

**INFO**  [d9-transaction-span-summary] Transaction span summary
  App: PT/D8 Tx Span 1.0.0.0  —  "D8BadSubscriber".HandlePosted()
  ws:src/subscriber.al:4  Commit at end of span
  coverage: complete