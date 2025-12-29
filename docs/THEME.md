# Themes

## Overview

`oyo` has two separate concepts:
- **UI theme**: colors for the chrome, diff markers, and UI elements.
- **Syntax theme**: colors for code tokens (tmTheme-based).

You can set them independently, but by default the syntax theme follows the UI theme
when you don't specify a syntax theme.

## UI Themes

Set a built-in UI theme:

```toml
[ui.theme]
name = "tokyonight"
```

Light/dark selection:

```toml
[ui.theme]
name = "tokyonight"
mode = "light" # or "dark"
```

List built-in UI themes:

```bash
oy themes
```

### Built-in UI Themes

| Theme | Dark | Light |
|-------|:----:|:-----:|
| aura | ✓ | — |
| ayu | ✓ | — |
| catppuccin | ✓ (mocha) | ✓ (latte) |
| catppuccin-frappe | ✓ | — |
| catppuccin-macchiato | ✓ | — |
| cobalt2 | ✓ | — |
| dracula | ✓ | — |
| everforest | ✓ | ✓ |
| flexoki | ✓ | ✓ |
| github | ✓ | ✓ |
| gruvbox | ✓ | ✓ |
| kanagawa | ✓ | — |
| material | ✓ | — |
| monokai | ✓ | — |
| nightowl | ✓ | ✓ |
| nord | ✓ | — |
| one-dark | ✓ | ✓ |
| palenight | ✓ | — |
| rosepine | ✓ | ✓ (dawn) |
| solarized | ✓ | ✓ |
| synthwave84 | ✓ | — |
| tokyonight | ✓ | ✓ (day) |
| zenburn | ✓ | — |

UI theme tokens are defined in [schema.json](crates/oyo/themes/schema.json).

### Custom UI themes

Place JSON theme files in either:

```
~/.config/oyo/MyTheme.json
~/.config/oyo/themes/MyTheme.json
```

Then reference them by file name (extension optional):

```toml
[ui.theme]
name = "MyTheme"
```

If you provide `MyTheme-light.json` and `MyTheme-dark.json`, `oyo` will pick the
variant based on `ui.theme.mode` (and fall back to the other if one is missing).

## Syntax Themes

Syntax highlighting is tmTheme-based. You can select a built-in syntax theme or provide
your own `.tmTheme` file.

```toml
[ui.syntax]
mode = "on"         # "on" or "off"
theme = "tokyonight"
```

Defaults:
- If `ui.syntax.theme` is empty, it inherits `ui.theme.name`.
- If it still can't be resolved, it falls back to `ansi`.

### Light variants

When `ui.theme.mode = "light"`, `oyo` tries a light variant first:
- `tokyonight` -> `tokyonight-day`
- `rosepine` -> `rosepine-dawn`
- `catppuccin` -> `catppuccin-latte`

Custom syntax themes can also provide `-light`/`-dark` variants (for example,
`cyberdream-light.tmTheme` and `cyberdream-dark.tmTheme`). `oyo` will pick the
appropriate variant based on `ui.theme.mode` and fall back to the other if needed.

You can also pick the variant explicitly:

```toml
[ui.syntax]
theme = "tokyonight-day"
```

### List syntax themes

```bash
oy syntax-themes
```

This lists:
- embedded syntax themes for built-in UI themes
- any `.tmTheme` files in `~/.config/oyo/themes`

### Custom tmTheme

Place a tmTheme file in:

```
~/.config/oyo/themes/MyTheme.tmTheme
```

Then reference it by name:

```toml
[ui.syntax]
theme = "MyTheme"
```

You can also pass a full path:

```toml
[ui.syntax]
theme = "/path/to/MyTheme.tmTheme"
```

If the file can't be loaded, `oyo` falls back to `ansi`.

### CLI overrides

```bash
oy --theme-name tokyonight --theme-mode light
oy --syntax-theme tokyonight-day
```

Note: syntax theme backgrounds are stripped to preserve the UI/diff background.
