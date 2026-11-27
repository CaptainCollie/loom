# loom

*Tool to create nested file and directory structures.*

## WORK IN PROGRESS

Create entries directly in the current directory with a root group:

```sh
loom "{new.rs, new/{new1.rs, new2.rs}}"
```

The outer braces are optional: `loom "new.rs, new/{new1.rs, new2.rs}"`
creates the same structure. Add `--dry-run` to preview it without creating files.

Run `loom` without a pattern to open the interactive file tree. Press `t` to
choose an icon color theme in the tree, or a syntax color theme while reading
a file. Press `Enter` to apply it, or `Esc` to cancel. Opening
the theme menu again refreshes the list. File icons use Nerd Fonts; configure
your terminal to use a compatible font.

## Icon themes

Loom includes a built-in Material icon color theme. To add another set,
download a ready-made **VSIX extension package** containing a VS Code file
icon theme and place the `.vsix` file in Loom's own theme directory:

| Platform | Directory |
| --- | --- |
| Windows | `%APPDATA%\loom\themes` |
| Linux | `$XDG_CONFIG_HOME/loom/themes` (usually `~/.config/loom/themes`) |
| macOS | `~/Library/Application Support/loom/themes` |

For example, add `catppuccin-vsc-icons-1.26.0.vsix`, open `t`, wait for the
list to load, and choose **Catppuccin Mocha**. All variants declared by a
package appear separately. Reopen the menu after adding or replacing a
package; unchanged packages are cached in memory. Esc cancels the menu and
pending imports. Packages are read in a background worker, without unpacking
or executing extension code.

The archive must contain `extension/package.json` with
`contributes.iconThemes` and the referenced theme manifests and assets.
Source-code ZIP archives, Zed themes, and standalone custom JSON themes are
not loaded. The last selected archive and variant are remembered in
`.selected-theme.json`. If that selection becomes unavailable, Loom uses
the built-in Material theme and reports the problem. Old `material.json`
selections map to the built-in theme; existing files are left untouched.

Loom uses colors from SVG fills, strokes, CSS, and gradients, or from a
font icon's `fontColor`. Glyph shapes still come from Nerd Fonts; this is
an approximation of a graphical icon set. Filenames keep their independent
style for future Git status colors. SVGs cannot read external resources.
PNG icons and unsupported paints use fallback colors with a summary warning.
Only the manifest's main association set is used; `light` and `highContrast`
overrides are not selected automatically. Language ID associations require
filename or extension mappings in the same package; VS Code's language
registry and extension-generated customizations are not available.

Manifests may contain comments and trailing commas. File names take
precedence over extensions, compound extensions over shorter ones, and
parent-qualified rules over ordinary rules of the same kind. VSIX rules
match case-insensitively. Expanded and root folder colors are supported.

Import limits: 16 MiB per manifest, 2 MiB per SVG, 128 MiB read per theme,
128 levels of JSON nesting, and 100,000 archive entries. Invalid packages remain visible with an error
and do not replace the active theme. No editor installation or runtime
network connection is required. Running a structure pattern does not load
themes.

## Syntax highlighting

Open a file with `Enter`. Loom detects its language from the filename,
extension, or shebang and highlights it with the built-in
`base16-ocean.dark` scheme. Supported grammars include Rust, Python,
JavaScript/TypeScript, JSON, TOML, YAML, Markdown, and shell. Unknown files
remain plain text. Highlighting is powered by syntect and two-face's embedded
grammars; no editor or language server is required.

To add colors, place a **color-theme VSIX** (for example, Catppuccin or One
Dark Pro) in the same `loom/themes` directory as icon packages. While reading,
press `t`, choose a variant, and press `Enter`. `No highlighting` disables
syntax colors. Esc closes the menu without changing the active theme.
Reopen the menu to discover added or replaced packages. The file position,
horizontal scroll, and search are preserved. Syntax selection is remembered
separately in `.selected-syntax-theme.json`; icon selection is independent.

Color packages use `contributes.themes`. JSONC, local `include` chains, and
TextMate `tokenColors` (including a packaged `.tmTheme` file) are supported.
Only the reading area's palette and token rules are applied. Semantic tokens,
language grammars from extensions, and the rest of VS Code's UI colors are
not imported, so rendering can differ from VS Code. Includes stay inside
the package, with cycle detection and a 16-file depth limit.

Text is displayed before syntax results are ready. A separate worker restores
multiline context from cached states; it never delays file navigation or
search. Syntax colors are applied first, the current line background next,
and search matches last. Tabs, Unicode, and horizontal clipping share the
text reader's source-to-display mapping.

Highlighting has its own limits: 64 KiB per source line, 8 MiB of context per
request, 8 MiB of style ranges, and 128 checkpoint positions including the
start. States are saved every 256 lines. A soft two-second budget is checked
between lines; an individual regex call cannot be interrupted. On a limit or
highlighting error, Loom shows plain text and a short notice. After an
oversized line it does not guess the missing multiline state. There is no
limit on total file size. Closing the file cancels pending highlighting.
