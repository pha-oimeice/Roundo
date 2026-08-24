---
status: accepted
---

# Render mod-registered UI through a Wry overlay

Roundo replaces its client-coupled Bevy UI with a Windows-first Wry child WebView whose UI Resources are registered by Mods. Traceable UI Resource Names are distinct from globally exclusive UI Registry Slots, so higher-priority Mods can replace a slot without impersonating another resource; each selected resource also declares hardware-input ownership and 3D-world visibility, while its internal visual state remains outside the client.
