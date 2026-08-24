# Roundo UI Lifecycle Tree — AI Implementation Specification

> Audience: implementation and review agents. This document is normative for the refactor. The concise human overview is [ui-lifecycle-tree-summary.markdown](ui-lifecycle-tree-summary.markdown); domain terminology is defined in [`/CONTEXT.md`](../../CONTEXT.md); the architectural decision is recorded in [ADR-0003](../adr/0003-ui-lifecycle-tree.markdown).

## 1. Purpose

Replace the current single-current-resource Web UI navigation model with a host-owned lifecycle tree. The new model must remove all page-authored knowledge of parent and return targets, preserve independent UI document state, permit sibling UI instances, and make server connection state—not whichever page happens to be open—the authority for camera and root lifetime.

This is not a browser history feature. It is ownership and lifetime management for Mod-provided UI resources.

## 2. Existing implementation being replaced

The current implementation has these load-bearing constraints:

- `roundo_client/roundo_webui/src/lib.rs::RoundoWebUiState` stores only `registry`, `current`, and `current_path`.
- `UiNavigationExecutor` owns one persistent `WebViewOverlay`; `ui.open` navigates that WebView in place.
- `roundo_client/src/ui_host.rs` derives player input and camera activation from the current resource's `interaction_mode`; `world_visibility` is registered but not applied by the host.
- Escape hard-codes `roundo.pause-menu` from an in-game resource.
- vanilla HTML hard-codes Back/Continue/Cancel targets with `ui.open`.
- `roundo_lifecycle::anchor_tree` is currently a private, inactive server ECS implementation. It mutates through deferred commands and does not implement atomic batch destruction, survival boundaries, cycle prevention, or validation suitable for UI lifetime ownership.

The refactor may reuse the anchor-tree domain semantics, but it must not couple the UI implementation to the existing ECS tick behavior.

## 3. Normative vocabulary

Use the canonical terms from `CONTEXT.md`. Particularly:

- **UI Definition**: immutable Mod registration for a kind of UI.
- **UI Instance**: one committed opening of a UI Definition.
- **Root Anchor**: non-visual root node for the active connection state.
- **Lifecycle Tree**: ownership graph rooted at exactly one Root Anchor.
- **Pending UI Open**: staged external resources not yet committed into the tree.
- **Lifecycle Destruction**: atomic mutation plan and commit.
- **Survival Boundary**: first independent descendant on a branch being destroyed.
- **Focused UI**: the one visible, Web-UI-interactive instance receiving input.
- **Recovery Surface**: host-owned error UI outside the registry/tree.

Do not call the lifecycle tree a navigation stack, return stack, render tree, or state-machine history.

## 4. Non-goals

The initial refactor must not:

1. expose instance IDs, parent IDs, root kinds, depth, or tree topology to page JavaScript;
2. add an opens/permissions graph between UI Definitions;
3. make opening, Back, or lifecycle destruction implicitly connect, cancel, retry, or disconnect networking;
4. introduce a global UI/WebView limit;
5. preserve UI instances across Root Replacement;
6. let a page veto destruction;
7. use browser cross-resource navigation as a substitute for `ui.open`;
8. make the generic lifecycle tree own Wry, Bevy camera, Mod registry, networking, focus, or presentation policy;
9. silently repair an internally invalid tree;
10. treat the host Recovery Surface as a Mod UI Instance.

## 5. Deep-module seams

### 5.1 Common Lifecycle Tree module

Create or extract a common, pure-data module whose interface hides parent/child bookkeeping and destruction traversal. The module must provide high leverage through a small interface. Suggested conceptual interface (names may follow repository conventions):

```rust
pub struct AnchorTree<N, M> { /* private */ }
pub struct DestructionPlan<N> { /* immutable validated result */ }
pub struct DestructionOutcome<N> {
    pub destroyed: Vec<N>,
    pub reparented: Vec<Reparented<N>>,
}

impl<N, M> AnchorTree<N, M>
where
    N: Copy + Eq + Hash,
{
    pub fn new(root: N, metadata: M) -> Self;
    pub fn root(&self) -> N;
    pub fn contains(&self, node: N) -> bool;
    pub fn parent(&self, node: N) -> Option<N>;
    pub fn children(&self, node: N) -> impl Iterator<Item = N>;
    pub fn insert_child(&mut self, parent: N, node: N, metadata: M) -> Result<(), TreeError>;
    pub fn plan_destruction(
        &self,
        explicit_targets: impl IntoIterator<Item = N>,
        root_is_unrestricted: bool,
        is_independent: impl Fn(N, &M) -> bool,
    ) -> Result<DestructionPlan<N>, TreeError>;
    pub fn commit(&mut self, plan: DestructionPlan<N>) -> DestructionOutcome<N>;
    pub fn validate(&self) -> Result<(), TreeInvariantError>;
}
```

This shape is illustrative, not permission to leak every internal operation. Prefer fewer methods if the same behavior can remain testable. The common module owns:

- exactly-one-root invariant;
- one-parent-per-non-root invariant;
- no cycles or self-parenting;
- deterministic child ownership;
- batch explicit targets;
- survival-boundary discovery;
- final reparent targets;
- plan validation and atomic commit;
- root-unrestricted destruction semantics.

It does not own instance counts or external resources; the UI adapter applies the returned outcome.

### 5.2 UI lifecycle manager

`roundo_webui` composes the common tree with:

- stable/generational `UiInstanceId` values;
- UI Definition lookup and process-wide live counts;
- `PendingUiOpen` state;
- one WebView adapter per committed instance;
- Root Anchor creation/replacement;
- focus history, visibility, z-order, layout, and presentation derivation;
- command source validation;
- Root UI selection and Recovery Surface state.

Callers should not coordinate tree mutation, counters, WebViews, and focus themselves. One manager interface should accept lifecycle intents and return structured outcomes/errors.

### 5.3 Adapters

Keep these as adapters at seams rather than leaking them into the lifecycle model:

- Wry/WebView2 creation, loading, bounds, focus, navigation filtering, protocol and IPC;
- Bevy schedule/state/messages;
- network connection event observation;
- camera/player input/cursor application;
- typed command dispatch;
- Mod registry TOML parsing.

## 6. State and root model

### 6.1 Source-defined state

The UI lifecycle state machine has exactly two source-defined semantic values:

```rust
pub enum UiLifecycleState {
    Disconnected,
    Connected,
}
```

Dynamic root IDs, UI Definition names, page paths, connection objects, and session handles do not belong in the enum.

### 6.2 Startup

At application startup:

1. the authoritative networking state is disconnected;
2. create a Disconnected Root Anchor;
3. resolve optional slot `roundo.disconnected-root`;
4. if absent, keep an empty tree and load no WebView;
5. if present, stage and load one ordinary Root UI child;
6. a malformed declared slot/resource remains a registry error, not an absent slot.

`roundo.initial` is removed. It is not retained as an alias.

### 6.3 Connected transition

A connection can be attempted only while networking is fully `Disconnected/Idle`. `Connecting`, `Connected`, and `Disconnecting` reject a new connect. Retry terminates or reuses the current attempt and must not create parallel sessions.

A Connecting UI remains in the Disconnected tree. Only an authoritative authenticated `Connected(session_handle)` event triggers Root Replacement:

1. immediately invalidate/cancel Pending UI Opens associated with the old root;
2. compute unrestricted destruction of the old root and every descendant;
3. atomically commit the new Connected Root Anchor bound internally to the authoritative session handle;
4. disable all old command endpoints/input and dispose old views;
5. resolve optional `roundo.connected-root`;
6. if present, stage its Root UI; if absent, load no Web UI;
7. Root UI loading failure shows the Recovery Surface but never rolls back connection or restores the Disconnected tree.

### 6.4 Disconnected transition

Only an authoritative disconnection event for the current connection triggers replacement with a Disconnected Root. The connection manager serializes connection ownership, so overlapping A/B sessions are not a supported state.

### 6.5 Camera and empty-root behavior

- Connected Root: game camera active regardless of loaded UI.
- Disconnected Root: game camera inactive regardless of UI registration.
- Connected and no Focused Web UI: game input active.
- Disconnected and no Focused Web UI: game input inactive.
- Root UI presence does not determine connection state or camera activation.

## 7. UI Definition schema

### 7.1 Required fields

Every `[[ui]]` entry retains existing identity/project/input/world fields and adds mandatory positive `max_instances`:

```toml
[[ui]]
name = "item-info"
project = "item-info"
entry = "index.html"
interaction_mode = "web-ui"
world_visibility = "visible"
max_instances = 8
lifecycle_independent = false
presentation = "concurrent"
layout = "windowed"
initial_width = 480
initial_height = 320
```

### 7.2 Defaults and validation

- `max_instances`: mandatory integer greater than zero; missing/zero/negative/out-of-range is invalid registry data.
- `lifecycle_independent`: defaults to `false`.
- `presentation`: defaults to `exclusive`; allowed values `exclusive`, `concurrent`.
- `layout`: defaults to `fullscreen`; allowed values `fullscreen`, `windowed`.
- `initial_width`, `initial_height`: required positive logical-pixel values for `windowed`; absent/rejected for impossible values. They are not required for `fullscreen`.
- Do not add global limits.

Presentation and layout are orthogonal; do not prohibit concurrent fullscreen or exclusive windowed registrations merely because common cases pair exclusive/fullscreen and concurrent/windowed.

### 7.3 Definition-level counting

Maintain a process-wide live count per fully qualified UI Definition name:

- slot aliases, paths, parents, and root kinds do not create count categories;
- count check happens when handling the current open request;
- staged WebViews consume no count;
- successful instance commit increments once;
- logical instance destruction decrements once, even if OS resource cleanup lags/fails;
- direct opening at the cap returns `ui_instance_limit` and never reuses/focuses an existing instance;
- lifecycle operations are serialized; no reservation or speculative concurrent counter protocol is required;
- a hot-reloaded lower maximum does not destroy existing instances, but new opens fail until the count drops below the maximum;
- Definition/Mod unloading first destroys its committed instances, then unregisters the definition.

## 8. Instance and identity model

Each committed instance needs at least:

```rust
struct UiInstance {
    id: UiInstanceId,
    definition: UiDefinitionId,
    current_path: String,
    webview: WebViewHandle,
    presentation: PresentationMode,
    layout: LayoutState,
    visibility: VisibilityState,
    focus_state: FocusState,
}
```

Use monotonically non-reused process IDs or generational IDs. Never let a delayed command for a destroyed instance address a later instance. IDs are host-internal and omitted from JS request/response payloads.

Root Anchors are tree nodes but not UI Instances. They have no WebView, Definition, path, presentation, counter, or page command endpoint.

## 9. Open protocol

### 9.1 Parent selection

- Web UI source: its bound source instance is the parent; the page cannot provide another parent.
- non-UI typed command/game system/test: default parent is current Root Anchor;
- trusted host internals may explicitly name a still-live parent instance or Root Anchor;
- any UI Definition may target any registered UI Definition or global slot; there is no opens graph.

### 9.2 Staged open

An open is not committed merely because `load_url` returned success. Required flow:

1. bind request to source context and root identity;
2. validate source, target slot/resource/path, definition registration, parent, schema, and current instance count;
3. create a new hidden, non-interactive staged WebView with its command endpoint disabled;
4. request target main-document navigation;
5. wait for successful main-document NavigationCompleted and injected bridge handshake;
6. before commit revalidate source still live, source still under the original Root Anchor, Definition still registered, and count still legal;
7. insert tree child, construct committed UI Instance, increment count once;
8. enable command endpoint;
9. derive visibility/z-order and focus the new instance;
10. return success without exposing its ID.

Failure before commit cleans up the staged WebView and leaves tree/counters/source presentation unchanged. Use a single host-configurable load timeout; Mods cannot override it.

### 9.3 Root interruption

Pending open I/O does not lock out authoritative root events. Root Replacement invalidates all pending opens from the old root and disposes their staged WebViews. A late completion cannot commit into the new root.

## 10. Lifecycle destruction algorithm

### 10.1 Explicit targets

A Back, native window close, host destroy, Mod unload, or shutdown supplies one or more explicit target nodes.

- Root Anchors cannot be targeted by ordinary `ui.back`.
- An ordinary Root UI child can Back and leave only its Root Anchor.
- A target's own `lifecycle_independent` never protects it.
- duplicate/overlapping targets are merged into one transaction.

### 10.2 Survival-boundary semantics

For a non-root explicit target `T` with surviving parent `P`, traverse each descendant branch:

1. the explicit target is destroyed;
2. a non-independent descendant remains in the destruction traversal;
3. the first independent descendant not itself explicitly targeted becomes a survival boundary;
4. reparent that boundary directly to the nearest ancestor outside the destruction set (normally `P`);
5. preserve the boundary's complete subtree unchanged with respect to ordinary cascading; descendant flags below it are not consulted merely because an ancestor outside the boundary is being destroyed;
6. explicit targets inside a surviving boundary's subtree remain mandatory and are processed as their own destruction roots—the survival boundary cannot shield them;
7. if the boundary itself is also an explicit target, destroy it and continue searching its descendant branches for later survival boundaries.

Example:

```text
P
└─ A(false, explicit target)
   └─ B(false)
      └─ C(true)
         └─ D(false)
```

Final result: A and B destroyed; C is reparented to P; D remains under C.

### 10.3 Root-unrestricted destruction

Root Replacement and app shutdown ignore all independent flags and destroy the Root Anchor plus every UI descendant. No UI Instance migrates between disconnected and connected roots.

### 10.4 Transaction and effects

Before exposing mutation:

- validate all explicit targets;
- calculate destroyed nodes exactly once;
- calculate final parent for every survivor;
- verify no cycle/missing parent/multiple parent/result without one root;
- calculate definition count deltas;
- calculate pending-open invalidations;
- calculate visibility/focus fallback.

Commit tree/counters/instance validity atomically. Immediately mark destroyed endpoints invalid. Afterwards perform best-effort notifications and OS resource disposal. Disposal failure logs diagnostics but never resurrects the logical instance or restores its counter.

### 10.5 Destruction notification

Inject non-cancellable host events:

- `roundo:visibility` with visible/hidden;
- `roundo:focus` with focused/blurred;
- `roundo:destroying` before external disposal.

Instances are marked destroying and command endpoints disabled before `roundo:destroying`; callbacks cannot issue host commands, delay, or veto destruction. Reliable data must be submitted earlier through ordinary business commands. App quit uses the same unrestricted root destruction best-effort but never waits for page cleanup.

## 11. Presentation model

### 11.1 Separation from lifecycle

Parentage answers ownership only. It does not directly define HWND z-order, browser history, visibility, or input focus. Presentation is recomputed from the committed tree and instance presentation metadata.

### 11.2 Exclusive

An exclusive instance hides its parent view and the parent's other visible descendant branches, while retaining their loaded WebViews and DOM state. Its own presentation subtree may remain visible. If several exclusive sibling branches exist, the most recently focused branch is visible; others remain loaded but hidden. Closing the active branch restores the most recent surviving branch.

### 11.3 Concurrent

A concurrent instance keeps its parent and eligible siblings visible. This permits an Inventory instance to own several simultaneously visible ItemInfo instances.

### 11.4 Layout

- Fullscreen bounds follow the Bevy client area's logical dimensions and DPI conversion.
- Windowed definitions provide initial logical width/height; initial position is centered.
- Per-instance moved/resized geometry is runtime presentation state and is not written back to Mod registration.
- Native window close maps to Lifecycle Destruction of that instance, exactly like `ui.back`.

### 11.5 Focus and z-order

- A successful open focuses the new instance and raises it above peers in its presentation group.
- Clicking a visible interactive WebView focuses and raises it.
- Closing/hiding a focused instance selects the most recently focused still-live visible candidate; otherwise the nearest visible ancestor; otherwise no Web UI focus.
- In Connected state, clicking an area outside all interactive WebView rectangles clears Web UI focus and restores game input.
- In Disconnected state, the same action clears focus without enabling game input.
- z-order and focus history never change lifecycle parentage.

### 11.6 Input

- `interaction_mode = "web-ui"`: visible viewport participates in WebView hit testing; focused instance receives keyboard input and disables game controls.
- `interaction_mode = "in-game"`: viewport is pointer-transparent/disabled for Web input; Connected state retains game controls.
- Only the Focused UI's interaction declaration decides whether Web UI owns input. Non-focused loaded windows do not disable game input by existence alone.

### 11.7 World visibility

`world_visibility` controls composition inside that instance's viewport only:

- `visible`: transparent WebView background permits active connected world rendering to show through;
- `hidden`: opaque clear/background prevents world rendering from showing through that viewport;
- outside a windowed rectangle, the field has no effect;
- it never activates/deactivates a camera;
- in Disconnected state there is no active 3D camera even when `visible`.

## 12. Commands and source context

### 12.1 Binding

Every WebView adapter owns/binds its `UiInstanceId`. IPC submission adds an unforgeable host-side source context before typed dispatch; never trust an ID sent in JSON. All WebView commands undergo admission validation:

- source instance still committed/live;
- source WebView endpoint still loaded/enabled;
- instance not destroying;
- command envelope valid.

Visibility and focus do not affect command validity. Hidden loaded UI timers may issue commands.

### 12.2 Accepted business commands

Once a non-lifecycle business command has passed admission and the destination service accepts it, later source UI destruction does not cancel it. Closing a Connecting UI does not cancel connecting; closing the UI that requested disconnect does not cancel disconnect.

### 12.3 Lifecycle commands

Expose to pages:

- `ui.open`: opens a child of the source instance;
- `ui.back`: explicitly destroys the source instance.

Do not expose arbitrary instance destroy, reparent, focus, parent selection, or tree query. `ui.open` returns success/error and target Definition information if useful, but no instance ID.

Suggested errors:

- `invalid_ui_resource`: missing/invalid target or path;
- `ui_instance_limit`: target Definition at `max_instances`;
- `stale_ui_instance`: source invalid before command execution/open commit;
- `cannot_close_ui_root`: attempted ordinary close of Root Anchor (host misuse; pages cannot address it);
- `ui_navigation_failed`: staged navigation/handshake failure;
- `ui_load_timeout`: staged load timeout;
- existing transport errors remain unchanged.

## 13. Navigation security

Top-level WebView navigation must not bypass lifecycle creation:

- same-Definition project-local path navigation is allowed within the same instance;
- successful same-Definition navigation updates `current_path` without changing ID, parent, count, or Definition;
- reload/history within that Definition remains one instance;
- top-level navigation to another resource/slot is rejected; use `ui.open`;
- cross-Definition frame/iframe loading is rejected;
- same-Definition local frames/assets are allowed subject to safe path resolution;
- external top-level URLs are cancelled;
- opening an external URL requires explicit `app.open-external-url`, protocol validation, and system-browser delegation.

Navigation/protocol source authority should come from the host-bound WebView/instance wherever the platform interface allows it, not solely from a spoofable page-provided identifier.

## 14. Escape and Back flows

- With a Focused Web UI, Escape is delivered to that page; the page may call `ui.back`.
- With no Focused Web UI in Connected state, the host resolves optional `roundo.pause-menu` and opens it.
- Pause parent is the most recently presented, live, visible `in-game` instance; if none exists, use Root Anchor.
- Missing pause slot logs an error and leaves state/input unchanged.
- pause Continue uses `ui.back`.
- pause Escape uses `ui.back`.
- settings Back uses `ui.back`.
- server-selection Back uses `ui.back`.
- Connecting Cancel explicitly invokes network cancellation/disconnect as a separate command; any subsequent UI close is not what cancels networking.
- Leave explicitly invokes disconnect; Root Replacement occurs only after authoritative Disconnected.

## 15. Recovery Surface

A Root UI navigation/handshake failure must not present the old root or falsify network state. Show a minimal host-owned, opaque Web UI input surface outside the Mod registry and tree. It:

- has no UI Definition or instance count;
- cannot call unrestricted ordinary Mod UI commands;
- provides controlled retry, disconnect where relevant, and quit actions;
- does not change camera state: Connected retains camera activity, though the surface visually covers the world;
- is replaced/removed when recovery succeeds or the root changes.

If WebView2 itself cannot be created, exit the client rather than pretend a Mod UI loaded.

## 16. Operation ordering

Lifecycle tree commits are serialized with deterministic priority for requests ready in the same update:

1. Root Replacement;
2. Definition/Mod forced unload;
3. instance destruction/Back/native close;
4. Pending open commit;
5. focus/window adjustment.

Async external page loading is not a tree mutation and cannot block Root Replacement. High-priority changes may make lower-priority requests stale. They never roll back the higher-priority fact.

## 17. Mod unloading and reload

Before removing UI Definitions:

1. batch all committed instances of removed Definitions as explicit destruction targets;
2. compute one atomic transaction;
3. removed-definition targets cannot survive through their own independence;
4. independent descendants owned by retained Definitions may reparent and survive;
5. cancel pending opens targeting or sourced from removed Definitions;
6. unregister only after committed counts reach zero for removed Definitions and logical destruction is committed.

## 18. Internal invariants and failure policy

At every committed state:

1. exactly one Root Anchor exists;
2. the root has no parent;
3. every UI Instance has exactly one live parent;
4. no node is its own ancestor;
5. every live UI Instance references a registered Definition;
6. every committed UI Instance has exactly one instance record and one WebView adapter;
7. each Definition live count equals committed instances of that Definition;
8. every focused instance is live, loaded, visible, and Web-UI-interactive;
9. every pending open references its source/root generation and consumes no live count;
10. disconnected and connected root trees never coexist.

Invalid page/Mod requests produce structured errors and no mutation. Internal invariant violations must emit sufficient diagnostics and fail fast; do not attempt ad-hoc orphan repair.

## 19. Vanilla migration

Update `mods/vanilla_ui/assets/webui/registry.toml`:

- remove `[general].initial`;
- map `roundo.disconnected-root` to main-menu;
- map `roundo.connected-root` to hud;
- add mandatory `max_instances` to every UI entry;
- add explicit independence/presentation/layout where behavior differs from defaults;
- HUD is fullscreen/in-game/world-visible;
- pause/settings/main menu/server selection/connecting are expected exclusive/fullscreen unless product UI later changes;
- item-information examples, when added, should be concurrent/windowed and use a deliberate instance maximum.

Update pages:

- settings Back → `ui.back`;
- server selection Back → `ui.back`;
- pause Continue/Escape → `ui.back`;
- remove hard-coded return-to-HUD/main-menu transitions;
- preserve explicit network Cancel/Leave commands, but do not manually open disconnected pages after disconnect; authoritative root replacement owns that visual transition;
- on connected status, do not repeatedly `ui.open` HUD; Connected Root Replacement loads the optional connected Root UI;
- handle structured command errors and prevent duplicate in-flight actions.

## 20. Validation contract

### 20.1 Pure tree tests

Test through the common lifecycle module interface:

- create root and children;
- reject unknown parent, duplicate node, self-parent, cycles, second root;
- destroy leaf;
- destroy non-root parent with ordinary descendants;
- direct independent child survival;
- deep survival boundary through ordinary ancestors;
- survival boundary preserves its complete subtree from ordinary cascading;
- an explicit target nested beneath a surviving boundary still dies;
- explicitly targeted independent node still dies;
- overlapping explicit targets are deduplicated;
- multiple branches and multiple survival boundaries;
- root-unrestricted destruction ignores all independence;
- planned result validates before commit;
- failed plan leaves original tree byte-for-byte/equivalently unchanged.

### 20.2 Registry tests

- optional disconnected/connected root slots;
- remove `roundo.initial` semantics;
- mandatory positive `max_instances`;
- defaults for independence/presentation/layout;
- windowed width/height validation;
- slot aliases share Definition count;
- missing optional root slot is valid;
- declared invalid root target is an error.

### 20.3 Lifecycle manager tests

- each open creates a distinct ID and increments count once;
- cap rejection is atomic;
- failed stage consumes no count and leaves tree unchanged;
- destruction decrements despite WebView disposal failure;
- source destroyed before stage commit causes stale failure;
- root replaced before stage commit cancels open;
- root UI Back leaves empty anchor;
- host open from empty anchor works;
- Definition unload preserves only independent retained-definition descendants;
- stale delayed command cannot target reused ID;
- source visibility/focus does not invalidate commands.

Use a fake WebView adapter; do not require Wry for pure lifecycle tests.

### 20.4 Presentation tests

- exclusive hides without unload;
- concurrent siblings remain visible;
- most-recent exclusive sibling wins;
- closing active exclusive restores prior branch;
- focus history fallback;
- clicking game background clears focus;
- in-game view pointer pass-through;
- camera derives only from root state;
- world visibility changes composition, not camera activation;
- surviving independent window retains geometry, DOM adapter identity, and visibility after reparent.

### 20.5 Command/navigation tests

- source context bound host-side;
- stale source rejected;
- accepted business command outlives source;
- `ui.back` destroys source, not a named parent target;
- UI cannot choose parent or receive instance ID;
- cross-resource top-level navigation rejected;
- cross-resource iframe rejected;
- same-resource path updates current path in place;
- external top-level navigation rejected;
- bridge handshake gates commit.

### 20.6 End-to-end domain scenarios

Primary scenario:

```text
ConnectedRoot
└─ HUD
   └─ Inventory
      ├─ ItemInfo A(false)
      ├─ ItemInfo B(true)
      └─ ItemInfo C(false)
```

After Inventory Back: Inventory/A/C destroyed; B subtree reparented to HUD; B WebView state retained; connection/camera unchanged. After Disconnected: old Root/HUD/B all destroyed; Disconnected Root created; optional disconnected Root UI loaded.

Also verify:

- main-menu → settings → Back returns to the still-live main-menu instance;
- HUD → pause → settings → Back returns to the still-live pause instance;
- entering settings never disconnects;
- closing connecting UI does not cancel connection;
- connection success while settings/connecting is open replaces the whole disconnected tree;
- connected settings keeps camera active while opaque presentation may hide the world.

### 20.7 Commands

At minimum run focused tests for the common lifecycle module, `roundo_webui`, `roundo_cli`, and client host. Run `cargo check` for affected workspace packages, then the broadest practical workspace test/check. Windows-specific WebView behavior needs a Windows manual or integration validation of multiple child WebViews, DPI bounds, z-order, focus, pointer transparency, navigation completion, and cleanup.

## 21. Suggested implementation workstreams

The source seams below are intended for orchestration; final ownership must avoid parallel writers in one checkout.

### Workstream A — pure lifecycle tree

- extract/move common lifecycle tree module;
- design deep tree interface;
- implement plan/validate/commit destruction;
- comprehensive pure unit tests;
- no Wry/UI/network dependencies.

### Workstream B — registry and UI lifecycle core

- extend schema and errors;
- replace single `current` state with Root Anchor/instances/pending opens/counts;
- integrate common tree interface;
- add fake WebView adapter test seam;
- implement Root UI optional slots and staged lifecycle operations.

### Workstream C — Windows multi-WebView presentation adapter

- one child WebView per instance;
- staged loading/bridge handshake;
- bounds, DPI, fullscreen/windowed, focus, z-order, transparency, pointer modes;
- command endpoint source binding;
- navigation/frame filtering and lifecycle events.

### Workstream D — client/network/commands/vanilla integration

- source-aware typed command context;
- `ui.back` and host open;
- authoritative root-state observation;
- camera/input split;
- Escape/pause behavior;
- vanilla registry/page migration;
- integration tests.

### Workstream E — review and validation

- fresh correctness review against this specification;
- focused tree/property tests;
- registry compatibility and migration review;
- Windows manual validation;
- final diff review for hard-coded parent targets and implicit network/UI coupling.

## 22. Completion criteria

The refactor is complete only when:

1. no vanilla Back/Continue flow opens a hard-coded parent UI;
2. settings opened from main menu and pause returns to the actual live parent;
3. settings does not alter connection state;
4. camera activation follows Connected/Disconnected rather than current UI interaction mode;
5. one Definition can have several committed sibling instances with independent WebViews up to its per-definition maximum;
6. the primary survival-boundary scenario passes;
7. Root Replacement destroys all descendants regardless of independence;
8. source-bound stale commands and cross-resource navigation bypasses are rejected;
9. optional root slots permit an empty Root Anchor;
10. focused tests and affected-package checks pass, with Windows-only residual validation explicitly reported if not automatable.
