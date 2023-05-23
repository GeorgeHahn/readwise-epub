# Readwise-Epub

This project packages up your Readwise Reader inbox into nicely grouped epubs. I
use it with a Kobo reader, but you should be able to use any device that can
open epub files.

## Installing / Prerequisites

Install [Rust](https://rustup.rs/) and
[Percollate](https://github.com/danburzo/percollate). These are both left as an
exercise to the reader.

Install this project by running `cargo install --path .` in the root directory.

Set your Readwise auth token by setting the environment variable
`READWISE_EPUB_READER_TOKEN` or writing it to a file in your [config
dir](https://docs.rs/dirs/latest/dirs/fn.config_dir.html)
`<config>/readwise-epub/config.toml` under the key `reader_token`.
