# Desktop layout

The desktop uses its existing system UI font, terminal monospace font and eight theme palettes. Selection and keyboard focus carry the accent; section headings, spacing and aligned controls establish hierarchy.

- Settings use a 24px content inset, 28px between groups and 16px row padding. Section headings use sentence case. Controls share a 280px column until the pane becomes narrow, then move below their labels. Full-width controls use the available pane width.
- Model catalogue rows use the same three grid tracks for name, context and prices. Optional badges and activity metadata stay inside the name cell. Price units appear below the catalogue. Provider credentials have persistent field labels.
- The catalogue and model details sit side by side above 800px of settings content width and stack below that. Endpoint details use labelled rows inside narrow detail panes; wide panes retain the table.
- UI secondary text is adjusted to at least 4.5:1 against the theme's five solid panel surfaces; ordinary secondary labels target 5:1. Theme hue is retained, and terminal ANSI colours are unchanged. Filled primary actions choose contrasting ink.
- Home, explorer, repository, menus, session previews, agent configuration and Windows-only setup dialogs use the same spacing principles. No terminal sizing or session behaviour changes are part of this pass.

Validation for 0.10.89: 98 frontend tests; Linux and Windows frontend builds; browser inspection of all nine shared settings sections at 1440px, 1000px and 640px window widths, plus all eight themes, provider forms, routing, activity, expanded Librarian, update access and the five agent configuration sections. The checked settings panes had no unintended horizontal overflow; catalogue numeric columns aligned across themes. Browser data was mocked, so these checks do not validate native installers, remote services or a physical Windows display.
