### ⛔ Transaction integrity — 2 high, 1 medium, 2 info

**HIGH**  [d35-commit-in-event-subscriber] Commit reachable from event subscriber
  App: PT/D8 Tx Span 1.0.0.0  —  "D8BadSubscriber".HandlePosted()
  ws:src/subscriber.al:4  [EventSubscriber] HandlePosted
  ws:src/subscriber.al:11  Commit
  coverage: complete

**HIGH**  [d8-commit-in-transaction] Commit inside a posting transaction span
  App: PT/D8 Tx Span 1.0.0.0  —  "D8BadSubscriber".HandlePosted()
  ws:src/posting.al:3  transaction-managing routine: PostSalesDoc
  ws:src/subscriber.al:11  Commit inside PostSalesDoc's transaction span
  coverage: complete

**MEDIUM**  [d3-missing-setloadfields] Missing SetLoadFields before a record retrieval
  App: PT/D8 Tx Span 1.0.0.0  —  "D8PostingChain".PostSalesDoc()
  ws:src/posting.al:8  Get on Header with no SetLoadFields
  ws:src/posting.al:9  accesses Header.Status Posted
  ws:src/posting.al:11  accesses Header.No.
  ws:src/posting.al:14  accesses Header.Last Posting Date
  coverage: complete

**INFO**  [d9-transaction-span-summary] Transaction span summary
  App: PT/D8 Tx Span 1.0.0.0  —  "D8BadSubscriber".HandlePosted()
  ws:src/subscriber.al:4  Commit at end of span
  coverage: complete

**INFO**  [d45-event-transitive-table-exposure] Event subscribers expose table transitively from publisher
  App: PT/D8 Tx Span 1.0.0.0  —  "D8PostingChain".OnAfterPostSalesDoc()
  ws:src/subscriber.al:4  subscriber writes 11111111-0000-0000-0000-00000000d80a/table/50100
  coverage: complete