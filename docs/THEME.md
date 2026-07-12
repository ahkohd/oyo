# Themes

Use themes to control Oyo colours.

Oyo has two theme types:

- UI themes control the chrome, diff markers and interface elements
- syntax themes control code token colours

You can set them separately. If you do not set a syntax theme, Oyo tries to use the UI theme name.

## Set a UI theme

Set a built-in UI theme in `config.toml`:

```toml
[ui.theme]
name = "tokyonight"
```

Set light or dark mode:

```toml
[ui.theme]
name = "tokyonight"
mode = "light" # or "dark"
```

List built-in UI themes:

```sh
oy themes
```

## Built-in UI themes

| Theme | Dark | Light |
| --- | --- | --- |
| `aura` | yes | no |
| `ayu` | yes | no |
| `catppuccin` | yes, mocha | yes, latte |
| `catppuccin-frappe` | yes | no |
| `catppuccin-macchiato` | yes | no |
| `cobalt2` | yes | no |
| `dracula` | yes | no |
| `everforest` | yes | yes |
| `flexoki` | yes | yes |
| `github` | yes | yes |
| `gruvbox` | yes | yes |
| `kanagawa` | yes | no |
| `material` | yes | no |
| `monokai` | yes | no |
| `nightowl` | yes | yes |
| `nord` | yes | no |
| `one-dark` | yes | yes |
| `palenight` | yes | no |
| `rosepine` | yes | yes, dawn |
| `solarized` | yes | yes |
| `synthwave84` | yes | no |
| `tokyonight` | yes | yes, day |
| `zenburn` | yes | no |

UI theme tokens are defined in [theme schema](../crates/oyo/themes/schema.json).

## Add a custom UI theme

Put JSON theme files in one of these locations:

```text
~/.config/oyo/MyTheme.json
~/.config/oyo/themes/MyTheme.json
```

Then use the file name in your config. The extension is optional.

```toml
[ui.theme]
name = "MyTheme"
```

You can provide light and dark variants:

```text
~/.config/oyo/themes/MyTheme-light.json
~/.config/oyo/themes/MyTheme-dark.json
```

Oyo picks the variant that matches `ui.theme.mode`. If one variant is missing, Oyo falls back to the other.

## Use ANSI colour names

Theme tokens can use ANSI colour names from your terminal palette.

```json
{
  "theme": {
    "diffAdded": { "dark": "green" },
    "diffRemoved": { "dark": "red" }
  }
}
```

Supported names are:

- `black`
- `red`
- `green`
- `yellow`
- `blue`
- `magenta`
- `cyan`
- `gray`
- `dark_gray`
- `light_red`
- `light_green`
- `light_yellow`
- `light_blue`
- `light_magenta`
- `light_cyan`
- `white`
- `default`
- `reset`
- `transparent`

## Set a syntax theme

Syntax highlighting uses TextMate `.tmTheme` files. Use a built-in syntax theme or provide your own file.

```toml
[ui.syntax]
mode = "on"         # "on" or "off"
theme = "tokyonight"
```

If `ui.syntax.theme` is empty, Oyo uses `ui.theme.name`. If Oyo still cannot resolve a syntax theme, it uses `ansi`.

## Set syntax warmup budgets

Use warmup budgets to control how much syntax highlighting Oyo prepares for large files.

```toml
[ui.syntax.warmup]
active_lines = 100
pending_lines = 300
idle_lines = 1000
debounce_ms = 80
```

## Use light syntax variants

When `ui.theme.mode = "light"`, Oyo tries a light syntax variant first.

| UI theme | Syntax theme Oyo tries first |
| --- | --- |
| `tokyonight` | `tokyonight-day` |
| `rosepine` | `rosepine-dawn` |
| `catppuccin` | `catppuccin-latte` |

Custom syntax themes can also provide light and dark variants. For example:

```text
cyberdream-light.tmTheme
cyberdream-dark.tmTheme
```

Oyo picks the variant that matches `ui.theme.mode`. If one variant is missing, Oyo falls back to the other.

You can also choose a variant directly:

```toml
[ui.syntax]
theme = "tokyonight-day"
```

## List syntax themes

Run:

```sh
oy syntax-themes
```

This lists:

- embedded syntax themes for built-in UI themes
- `.tmTheme` files in `~/.config/oyo/themes`

## Add a custom syntax theme

Put a `.tmTheme` file in:

```text
~/.config/oyo/themes/MyTheme.tmTheme
```

Then use the theme name:

```toml
[ui.syntax]
theme = "MyTheme"
```

You can also pass a full path:

```toml
[ui.syntax]
theme = "/path/to/MyTheme.tmTheme"
```

If Oyo cannot load the file, it falls back to `ansi`.

## Override themes from the CLI

```sh
oy --theme-name tokyonight --theme-mode light
oy --syntax-theme tokyonight-day
```

Oyo strips syntax theme backgrounds so the UI and diff backgrounds stay consistent.
