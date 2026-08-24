# Use an ownership tree for UI lifecycles

Roundo will model client UI lifecycles as a host-owned tree rooted in the authoritative disconnected or connected state, rather than as a single-WebView navigation state or page-authored return graph. Every opening creates an independently counted UI instance; ordinary ancestor destruction cascades until lifecycle-independent survival boundaries are reparented, while root replacement destroys the whole tree. Presentation, focus, WebView ownership, and network connection remain separate adapters around this tree so pages never need to know their parent or connection tree.

## Considered Options

A browser-style return stack cannot represent several sibling item-information windows or independently surviving descendants. Keeping parent targets in Mod pages preserves the current coupling and makes settings return behavior depend on where it was hard-coded. Reusing the current server ECS anchor systems directly would expose deferred-tick behavior and lacks the atomic batch operations required by UI resource ownership.

## Consequences

Each committed UI instance owns a WebView and definition-level instance count, making concurrent UI state independent but more resource-intensive. Root UI slots are optional, connection facts take precedence over visual loading, and a host recovery surface is required when a configured Root UI cannot load.
