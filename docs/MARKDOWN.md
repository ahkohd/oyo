# Markdown preview test

Use this file to check Oyo preview mode. Open this file in Oyo, switch to preview mode, then use the top-right toggle to compare preview and source.

## Paragraphs

This is a short paragraph with plain text. It should wrap cleanly and keep enough spacing between sections.

This paragraph includes **strong text**, *emphasis*, ***strong emphasis***, `inline code`, ~~deleted text~~, a [link to the Oyo repository](https://github.com/ahkohd/oyo), and a footnote reference.[^note]

## Headings

# Heading 1

## Heading 2

### Heading 3

#### Heading 4

##### Heading 5

###### Heading 6

## Block quotes

> A block quote should stand apart from normal text.
>
> It can contain more than one paragraph.

> [!NOTE]
> GitHub-style callouts should still be readable if they render as normal quotes.

## Lists

Unordered list:

- first item
- second item with `code`
- third item
  - nested item
  - another nested item

Ordered list:

1. open Oyo
2. switch to preview mode
3. compare source and preview

Task list:

- [x] render headings
- [x] render lists
- [ ] render every Markdown extension perfectly

## Table

| Setting | Value | Notes |
| --- | --- | --- |
| view_mode | preview | Shows rendered Markdown or source text |
| scrollbar | true | Shows the preview scrollbar |
| line_wrap | false | Source text uses the current wrapping setting later |

## Code

Inline code should stay inline: `cargo test -q`.

```rust
fn main() {
    println!("hello from markdown preview");
}
```

```toml
[ui]
view_mode = "preview"
scrollbar = true
```

Indented code:

    let value = 42;
    assert_eq!(value, 42);

## Rules

---

## Images and HTML

![Oyo preview swatch](./assets/preview.png)

<kbd>tab</kbd> changes view mode.

## Definition list

Preview mode
: Shows Markdown as rendered text or source text.

Source mode
: Shows the Markdown file with syntax highlighting.

## Footnotes

[^note]: This is a footnote used to test footnote rendering.
