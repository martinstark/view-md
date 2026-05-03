# Code-early test

A doc with a code block in the first viewport — exercises the path
where viewport-aware skip-wait must NOT skip (because a placeholder
code block would be visible in the initial paint).

## Quick example

Here is a small Rust snippet right at the top of the page:

```rust
fn fibonacci(n: u32) -> u64 {
  match n {
    0 => 0,
    1 => 1,
    _ => fibonacci(n - 1) + fibonacci(n - 2),
  }
}

fn main() {
  for i in 0..10 {
    println!("fib({}) = {}", i, fibonacci(i));
  }
}
```

And a Python equivalent for comparison:

```python
def fibonacci(n):
    if n < 2:
        return n
    return fibonacci(n - 1) + fibonacci(n - 2)

if __name__ == "__main__":
    for i in range(10):
        print(f"fib({i}) = {fibonacci(i)}")
```

## More content

Some prose to push the rest of the page below the first viewport so
this file roughly mirrors the structure of a typical README — code
near the top, body text following.

The first viewport at the default 920×1100 window should contain the
title, this section's heading, the Rust block, the Python block, and
maybe a few lines of this paragraph. If viewport-aware skip-wait is
working correctly, the syntect wait should fire here (because a
placeholder code block IS visible), avoiding a placeholder→highlighted
flash.

## Bash and JavaScript

A few more languages further down to give the syntect precompute
something to chew on:

```bash
#!/usr/bin/env bash
set -euo pipefail
for f in "$@"; do
  if [[ -f "$f" ]]; then
    wc -l "$f"
  fi
done
```

```javascript
const fib = (n) => (n < 2 ? n : fib(n - 1) + fib(n - 2));
console.log([...Array(10).keys()].map(fib));
```

## Tail

Filler. Filler. Filler. So the doc isn't just code blocks. The
viewport-aware check only inspects the first 1100px of the laid doc,
so anything below this point is irrelevant to the skip decision but
still gets highlighted by the post-paint async upgrade.
