---
status: accepted
---

# Use typed JSON for client commands

Roundo exposes client capabilities as versioned typed JSON commands with typed JSON results, processed through a source-agnostic request/response seam. Rust command definitions own the input and output schemas; commands may opt into a generated Unix projection through `UnixCommand`, allowing terminals to remain familiar without making Unix text the client interface or duplicating command semantics.
