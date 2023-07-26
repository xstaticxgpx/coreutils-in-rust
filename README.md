# ratiscat

`rat` is a performance oriented re-implementation of `cat` in rust

See [`rat.rs`](/src/bin/rat.rs)

For a fully fledged coreutils rust stack see `uutils`:

https://github.com/uutils/coreutils/

### Usage

`rat` vs. `cat`

```
$ rat <t/test.rand | pv -r >/dev/null
[2.03GiB/s]

$ cat <t/test.rand | pv -r >/dev/null
[2.04GiB/s]

$ echo test | ./rat - /does/not/exists /etc/hosts /does/not/exists2 | md5sum
rat: /does/not/exists: No such file or directory
rat: /does/not/exists2: No such file or directory
27f2e6689a97a42813e55d44ef29cda4  -

$ echo test | cat - /does/not/exists /etc/hosts /does/not/exists2 | md5sum
cat: /does/not/exists: No such file or directory
cat: /does/not/exists2: No such file or directory
27f2e6689a97a42813e55d44ef29cda4  -

```

### Motivation

I just wanted to do this as a learning experience for rust.

At least, that's how it started.

Things learned in relation to rust:

- `Stdout` in rust will always be wrapped by `LineWriter` which flushes on new line

Wrap the raw file descriptor in `BufWriter<File>` instead for more control

- Pre-allocation given a Sized `Vec<u8>` for buffer has some interesting impacts on runtime performance

Even when that vector is immediately cleared on runtime the behavior between _initially empty_ vs. _initially Sized_ vector is noticeable


### Known Bugs

- Ctrl+D needs to be pressed twice while interactive
