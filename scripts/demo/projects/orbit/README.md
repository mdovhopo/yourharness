# orbit

A tiny static site generator. Markdown files in `content/` become HTML pages in `public/`.

```sh
cargo run -- build          # content/ → public/
cargo run -- serve 4000     # build, then serve public/
```

Front matter between `---` lines sets `title` and `date`.
