# loom

*Tool to create nested file and directory structures.*

## WORK IN PROGRESS

Run `loom` without a pattern to open the interactive file tree. Press `t` to
choose an icon color theme, `Enter` to apply it, or `Esc` to cancel. Opening
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

### Maintaining the built-in theme

The Python converter is a development tool for regenerating the bundled
Material data from its pinned local Zed source tree:

```sh
python tools/prepare_icon_theme.py path/to/material-icon-theme-source
```

Preparation requires Python 3.10+; normal builds and runtime do not. Source
revisions, color-conversion limitations, and licenses are recorded in
`assets/icon-themes/`.
