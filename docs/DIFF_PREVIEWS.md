# Diff configuration previews

Use these previews to choose diff display settings.

Each example shows the TOML you need for `[ui.diff]`, followed by a screenshot.

## Theme foreground

Use `fg = "theme"` to colour diff text with the UI theme.

### Theme foreground without line backgrounds

#### No inline highlight

```toml
[ui.diff]
fg = "theme"
bg = false
highlight = "none"
```

![theme foreground, no line background, no inline highlight](../assets/fg_theme_bg_false_hi_none.png)

#### Word highlight

```toml
[ui.diff]
fg = "theme"
bg = false
highlight = "word"
```

![theme foreground, no line background, word highlight](../assets/fg_theme_bg_false_hi_word.png)

#### Text highlight

```toml
[ui.diff]
fg = "theme"
bg = false
highlight = "text"
```

![theme foreground, no line background, text highlight](../assets/diff_fg_theme_bg_false_hi_text.png)

### Theme foreground with line backgrounds

#### No inline highlight

```toml
[ui.diff]
fg = "theme"
bg = true
highlight = "none"
```

![theme foreground, line background, no inline highlight](../assets/fg_theme_bg_true_hi_none.png)

#### Text highlight

```toml
[ui.diff]
fg = "theme"
bg = true
highlight = "text"
```

![theme foreground, line background, text highlight](../assets/fg_theme_bg_true_hi_text.png)

#### Word highlight

```toml
[ui.diff]
fg = "theme"
bg = true
highlight = "word"
```

![theme foreground, line background, word highlight](../assets/fg_theme_bg_true_word.png)

## Syntax foreground

Use `fg = "syntax"` to keep syntax colours in diff text.

### Syntax foreground without line backgrounds

#### No inline highlight

```toml
[ui.diff]
fg = "syntax"
bg = false
highlight = "none"
```

![syntax foreground, no line background, no inline highlight](../assets/fg_syntax_bg_false_hi_none.png)

#### Text highlight

```toml
[ui.diff]
fg = "syntax"
bg = false
highlight = "text"
```

![syntax foreground, no line background, text highlight](../assets/fg_syntax_bg_false_hi_text.png)

#### Word highlight

```toml
[ui.diff]
fg = "syntax"
bg = false
highlight = "word"
```

![syntax foreground, no line background, word highlight](../assets/fg_syntax_bg_false_hi_word.png)

### Syntax foreground with line backgrounds

#### No inline highlight

```toml
[ui.diff]
fg = "syntax"
bg = true
highlight = "none"
```

![syntax foreground, line background, no inline highlight](../assets/fg_syntax_bg_true_hi_none.png)

#### Text highlight

```toml
[ui.diff]
fg = "syntax"
bg = true
highlight = "text"
```

![syntax foreground, line background, text highlight](../assets/fg_syntax_bg_true_hi_text.png)

#### Word highlight

```toml
[ui.diff]
fg = "syntax"
bg = true
highlight = "word"
```

![syntax foreground, line background, word highlight](../assets/fg_syntax_bg_true_hi_word.png)

## Extent markers

Use extent markers to show the current hunk or step area.

### Diff-coloured extent marker

```toml
[ui.diff]
extent_marker = "diff"
```

![diff-coloured extent marker](../assets/diff_extent_marker_diff.png)

### Hunk scope

```toml
[ui.diff]
extent_marker_scope = "hunk"
```

![extent marker scoped to hunk](../assets/extent_marker_scope_hunk.png)

### Progress scope

```toml
[ui.diff]
extent_marker_scope = "progress"
```

![extent marker scoped to progress](../assets/extent_marker_scope_progress.png)

## Gutter signs

Use `gutter_signs = false` to hide the sign column in unified and evolution views.

### No line backgrounds

```toml
[ui]
gutter_signs = false

[ui.diff]
bg = false
```

![gutter signs disabled without line backgrounds](../assets/bg_false_gutter_signs_false.png)

### Line backgrounds

```toml
[ui]
gutter_signs = false

[ui.diff]
bg = true
```

![gutter signs disabled with line backgrounds](../assets/bg_true_gutter_signs_false.png)

## Split view line alignment

Use `align_lines = true` to insert blank rows so old and new panes stay aligned.

### Without line alignment

```toml
[ui.split]
align_lines = false
```

![split view without line alignment](../assets/align_lines_false.png)

### With line alignment

```toml
[ui.split]
align_lines = true
```

![split view with line alignment](../assets/align_lines_true.png)

## Syntax highlighting off

Use this when you want plain diff colours without syntax highlighting.

```toml
[ui.syntax]
mode = "off"
```

![syntax highlighting off](../assets/ui_syntax_off.png)
