# mdv smoke test

A minimal document to verify rendering.

## Inline formatting

This is **bold**, this is *italic*, this is ~~struck through~~, and this is `inline code`.

A link to [example.com](https://example.com) — clicking should open the system browser.

## Lists

- one
- two
  - nested
  - also nested
- three

1. first
2. second
3. third

- [x] checked task
- [ ] unchecked task

## Quote

> The best way out is always through.
> — Robert Frost

## Table

| lang | year | paradigm     |
| ---- | ---- | ------------ |
| Rust | 2010 | systems      |
| Go   | 2009 | systems      |
| TS   | 2012 | structural   |

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

## Footnote

Here is a sentence with a footnote.[^1]

[^1]: And here is the footnote body.
