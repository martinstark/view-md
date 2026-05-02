# vmd smoke test

A minimal document to verify rendering.

## Inline formatting

This is **bold**, this is *italic*, this is ~~struck through~~, and this is `inline code`.

A link to [example.com](https://example.com) — clicking should open the system browser.

Mixed: ***bold italic***, **bold with `code` inside**, ~~strike with *italic*~~, [a link with **bold** *italic* `code`](https://example.com).

## Lists

- one
- two
  - nested
  - also nested
    - deeper
    - deeper still
- three

1. first
2. second
3. third

- [x] checked task
- [ ] unchecked task

### Long ordered list

1. first
2. second
3. third
4. fourth
5. fifth
6. sixth
7. seventh
8. eighth
9. ninth
10. tenth
11. eleventh
12. twelfth
13. thirteenth
14. fourteenth
15. fifteenth
16. sixteenth
17. seventeenth
18. eighteenth
19. nineteenth
20. twentieth

## Quote

> The best way out is always through.
> — Robert Frost

## Table

| lang | year | paradigm     |
| ---- | ---- | ------------ |
| Rust | 2010 | systems      |
| Go   | 2009 | systems      |
| TS   | 2012 | structural   |

## Images

A reference to a local file: ![vmd icon](../assets/icon.png).

Inline in a sentence: hello ![logo](../assets/icon.png) world.

A remote URL (not fetched in v1): ![remote example](https://example.com/img.png).

## Code

```rust
fn main() {
    let xs: Vec<i32> = (1..=5).collect();
    let sum: i32 = xs.iter().sum();
    println!("sum = {sum}");
}
```

```ts
const greet = (name: string): string => `hello, ${name}`;
console.log(greet("world"));
```

```python
def fib(n: int) -> int:
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a
```

### Longer code block

```rust
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

#[derive(Debug, Clone)]
struct Document {
    title: String,
    content: Vec<String>,
    metadata: HashMap<String, String>,
}

impl Document {
    fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    fn from_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref();
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut doc = Self::new(title);
        for line in reader.lines() {
            doc.content.push(line?);
        }
        Ok(doc)
    }

    fn word_count(&self) -> usize {
        self.content
            .iter()
            .flat_map(|line| line.split_whitespace())
            .count()
    }

    fn save<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let mut file = File::create(path)?;
        writeln!(file, "# {}", self.title)?;
        for (k, v) in &self.metadata {
            writeln!(file, "{}: {}", k, v)?;
        }
        for line in &self.content {
            writeln!(file, "{}", line)?;
        }
        Ok(())
    }
}

fn main() -> io::Result<()> {
    let mut doc = Document::from_file("input.txt")?;
    doc.metadata.insert("created".into(), "2026-05-02".into());
    println!("loaded {} ({} words)", doc.title, doc.word_count());
    doc.save("output.md")
}
```

### Code containing markdown-ish text

```bash
# a comment containing **bold** and *italic* markers
echo "and a string with `not` inline code"
sed -i 's/\[link\](.*)//g' file.md
grep -E "^- " | wc -l
```

### Code with programming ligatures

```ts
const arrows = (x: number): number => x + 1;
if (a !== b && a >= 0 && b <= 100) { /* ... */ }
const pipe = <A, B, C>(f: (a: A) => B, g: (b: B) => C) => (a: A) => g(f(a));
```

## Mixed layouts

### List items containing structure

1. Plain text item.
2. Second item with **bold**, *italic*, and a [link](https://example.com).
3. Third item containing a code block:
   ```python
   print("indented inside list item")
   ```
4. Fourth item with a nested list:
   - sub-item with `inline code`
   - sub-item with [a link](https://example.com)
     1. deeper ordered
     2. deeper still
5. Fifth item with a quote inside:
   > Quotes nest unexpectedly well inside loose lists.
6. Sixth and last.

### Blockquote containing structure

> A quote that opens with prose, then drops into a list:
>
> - first nested
> - second nested with `code`
>
> followed by another paragraph,
>
> ```rust
> fn quoted() -> u32 { 42 }
> ```
>
> and a small table:
>
> | k | v |
> | - | - |
> | a | 1 |
> | b | 2 |

### Inline mixing extremes

A paragraph with **bold _italic with `code` inside_ continued bold** and ~~strike with [a link](https://example.com) inside~~.

A link containing other formatting: [**bold link**, *italic link*, and `code link`](https://example.com).

### Tight wrap and overflow

A really-long-unbroken-identifier-that-could-overflow-narrow-columns: `verylongidentifierwithoutbreakopportunities_keep_going_keep_going_keep_going`.

| short | longer | a really really really long column header |
| ----- | ------ | ----------------------------------------- |
| a     | b      | a longer cell value to test wrapping       |
| 1     | 22     | another row with content that flows on     |

### Successive headings

#### h4 directly above

##### h5 with no body

###### h6 nested deepest

then a body paragraph immediately under h6.

### Adjacent rules and empty content

---

---

A paragraph between two more rules.

---

### Ordered list starting mid-count

5. starts at five
6. continues
7. and ends here

## Footnote

Here is a sentence with a footnote.[^1] And another reference[^edge] later in the doc.

[^1]: And here is the footnote body.

[^edge]: A second footnote, demonstrating the definition list at the end.
