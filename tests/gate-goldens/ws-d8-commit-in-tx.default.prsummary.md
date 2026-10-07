### ⛔ Transaction integrity — 1 high, 2 info

**HIGH**  [d35-commit-in-event-subscriber] Commit reachable from event subscriber
  App: PT/D8 Tx Span 1.0.0.0  —  "D8BadSubscriber".HandlePosted()
  ws:src/subscriber.al:4  [EventSubscriber] HandlePosted
  ws:src/subscriber.al:11  Commit
  coverage: complete

**INFO**  [d9-transaction-span-summary] Transaction span summary
  App: PT/D8 Tx Span 1.0.0.0  —  "D8BadSubscriber".HandlePosted()
  ws:src/subscriber.al:4  Commit at end of span
  coverage: complete

**INFO**  [d45-event-transitive-table-exposure] Event subscribers expose table transitively from publisher
  App: PT/D8 Tx Span 1.0.0.0  —  "D8PostingChain".OnAfterPostSalesDoc()
  ws:src/subscriber.al:4  subscriber writes 11111111-0000-0000-0000-00000000d80a/table/50100
  coverage: complete