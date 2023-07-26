# ratiscat

`rat` is a performance oriented re-implementation of `cat` in rust

See [`rat.rs`](/src/bin/rat.rs)

For a fully fledged coreutils rust stack see `uutils`:

https://github.com/uutils/coreutils/

### Usage

`rat` vs. `cat`

```
# Regular file to pipe (dd if=/dev/urandom of=test.rand bs=1MB count=4096)
$ rat <test.rand | pv -r >/dev/null
[2.69GiB/s] # <--- How is this faster than cat?
            # `cat` writes 2^17 (128KB) buffer size by default
            # `rat` detects FIFO in or out and uses 2^16 (64KB)
            # which results in higher throughput across the pipe
            # compare with `strace --syscall-limit=100` in front

$ cat <test.rand | pv -r >/dev/null
[2.09GiB/s]

# Pipe to pipe
$ timeout -s SIGINT 5 yes | rat | pv -r >/dev/null
[2.85GiB/s]

$ timeout -s SIGINT 5 yes | cat | pv -r >/dev/null
[2.86GiB/s]

# Argument ordering, error logging
$ echo test | rat - /does/not/exists /etc/hosts /does/not/exists2 | md5sum
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

  Even when that vector is immediately cleared on runtime the behavior between _initially empty_ vs. _initially Sized_ vector is noticeable.
  Alternatively `Vec.with_capacity` works too.


### Known Bugs

- Ctrl+D needs to be pressed twice while interactive
