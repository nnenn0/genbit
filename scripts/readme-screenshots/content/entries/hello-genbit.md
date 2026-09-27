+++
title = "Hello, genbit"
description = "genbit turns Markdown and Tera templates into a small static site."
created_at = 2026-09-26 09:00
updated_at = 2026-09-26 09:00
tags = ["genbit", "rust"]
+++

genbit turns Markdown and Tera templates into a small static site. Everything is configured in a single `config.toml`.

## Build the site

Run the build from the site root. Everything lands in `dist/`, ready for any static host.

```sh
genbit build
```

## Preview while you write

`genbit dev` rebuilds on every save and reloads the browser over SSE.

```rust
fn main() {
    println!("Hello, genbit!");
}
```
