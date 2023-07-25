# ratiscat

`rat` is a performance oriented re-implementation of `cat` in rust

See [`rat.rs`](/src/bin/rat.rs)

For a fully fledged coreutils rust stack see `uutils`:
https://github.com/uutils/coreutils/

### Motivation

I just wanted to do this as a learning experience for rust.

At least, that's how it started.

Things learned in relation to rust:

- `Stdout` in rust will always be wrapped by `LineWriter` which flushes on new line

Wrap the raw file descriptor in `BufWriter<File>` instead for more control

- Pre-allocation given a Sized `Vec<u8>` for buffer has some interesting impacts on runtime performance

Even when that vector is immediately cleared on runtime the behavior between _initially empty_ vs. _initially Sized_ vector is noticeable
