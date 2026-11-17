# ScratchRunner file navigator

A reusable macOS NSH file navigator. The embedding component owns the navigator state and chooses the initial directory. The navigator displays real files, opens them through a callback, and lists folders lazily on worker threads.

## Motion inventory

The existing ScratchRunner design specification describes the navigator in `DESIGN.md` §9.3, Sidebar. Row hover and selection use the instant wash; disclosure rotates with the base transition; folder drop target uses the instant outline; inline rename appears in place. Filtering updates immediately while typing. The navigator has no continuous animation or sound. Reduced motion makes changes instant. Native menus use platform motion. Dragging follows the pointer directly. The appearance of created and removed rows and animated layout transitions still need native verification in the integrated application.
