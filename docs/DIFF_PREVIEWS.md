# Diff Configuration Previews

## Foreground: Theme

### bg = false

#### highlight = "none"

```toml
[ui.diff]
fg = "theme"
bg = false
highlight = "none"
```

![fg=theme, bg=false, highlight=none](../assets/fg_theme_bg_false_hi_none.png)

#### highlight = "word"

```toml
[ui.diff]
fg = "theme"
bg = false
highlight = "word"
```

![fg=theme, bg=false, highlight=word](../assets/fg_theme_bg_false_hi_word.png)

#### highlight = "text"

```toml
[ui.diff]
fg = "theme"
bg = false
highlight = "text"
```

![fg=theme, bg=false, highlight=text](../assets/diff_fg_theme_bg_false_hi_text.png)

### bg = true

#### highlight = "none"

```toml
[ui.diff]
fg = "theme"
bg = true
highlight = "none"
```

![fg=theme, bg=true, highlight=none](../assets/fg_theme_bg_true_hi_none.png)

#### highlight = "text"

```toml
[ui.diff]
fg = "theme"
bg = true
highlight = "text"
```

![fg=theme, bg=true, highlight=text](../assets/fg_theme_bg_true_hi_text.png)

#### highlight = "word"

```toml
[ui.diff]
fg = "theme"
bg = true
highlight = "word"
```

![fg=theme, bg=true, highlight=word](../assets/fg_theme_bg_true_word.png)

## Foreground: Syntax

### bg = false

#### highlight = "none"

```toml
[ui.diff]
fg = "syntax"
bg = false
highlight = "none"
```

![fg=syntax, bg=false, highlight=none](../assets/fg_syntax_bg_false_hi_none.png)

#### highlight = "text"

```toml
[ui.diff]
fg = "syntax"
bg = false
highlight = "text"
```

![fg=syntax, bg=false, highlight=text](../assets/fg_syntax_bg_false_hi_text.png)

#### highlight = "word"

```toml
[ui.diff]
fg = "syntax"
bg = false
highlight = "word"
```

![fg=syntax, bg=false, highlight=word](../assets/fg_syntax_bg_false_hi_word.png)

### bg = true

#### highlight = "none"

```toml
[ui.diff]
fg = "syntax"
bg = true
highlight = "none"
```

![fg=syntax, bg=true, highlight=none](../assets/fg_syntax_bg_true_hi_none.png)

#### highlight = "text"

```toml
[ui.diff]
fg = "syntax"
bg = true
highlight = "text"
```

![fg=syntax, bg=true, highlight=text](../assets/fg_syntax_bg_true_hi_text.png)

#### highlight = "word"

```toml
[ui.diff]
fg = "syntax"
bg = true
highlight = "word"
```

![fg=syntax, bg=true, highlight=word](../assets/fg_syntax_bg_true_hi_word.png)

## Other Options

### Extent Markers

#### extent_marker_scope = "hunk"

![extent marker scope hunk](../assets/extent_marker_scope_hunk.png)

#### extent_marker_scope = "progress"

![extent marker scope progress](../assets/extent_marker_scope_progress.png)

#### extent_marker = "diff"

![extent marker diff](../assets/diff_extent_marker_diff.png)

### Gutter Signs

#### bg = false, gutter_signs = false

![gutter signs disabled, bg=false](../assets/bg_false_gutter_signs_false.png)

#### bg = true, gutter_signs = false

![gutter signs disabled, bg=true](../assets/bg_true_gutter_signs_false.png)

### Split View: Align Lines

#### align_lines = false

![align_lines false](../assets/align_lines_false.png)

#### align_lines = true

![align_lines true](../assets/align_lines_true.png)

### Syntax Highlighting

#### syntax = "off"

![syntax off](../assets/ui_syntax_off.png)
