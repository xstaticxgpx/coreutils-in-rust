# ratiscat

`rat` is a performant re-implementation of `cat` in rust

Turns out a rat fits in a pipe better than a cat anyways.

![Baba is You Cat is Jelly](img/baba_cat_is_jelly.png)

See [`rat.rs`](/src/bin/rat.rs)

Also checkout the `uutils` project:

https://github.com/uutils/coreutils/

### Usage

As run on and compared to: i7-6800K / 128GB DDR4 (file reads cached) / Linux 6.3.9 / btrfs (CoW) / coreutils 9.3 / uutils 0.0.20

#### Basic functionality (raw I/O)

```
# 4GB random data sample
$ dd if=/dev/urandom of=test.rand bs=1MB count=4096

# file to file

$ time rat test.rand >test.$(date +%s)
real    0m1.715s # <-- io::copy uses sendfile() first then copy_file_range() :(
user    0m0.001s
sys     0m1.710s
$ time cat test.rand >test.$(date +%s)
real    0m0.004s # <-- cat uses copy_file_range() all the time, great for btrfs
user    0m0.004s
sys     0m0.000s

# file to pipe

$ rat test.rand | pv -r >/dev/null
[2.69GiB/s] # <-- rat automagically configures size for FIFO pipes
$ cat test.rand | pv -r >/dev/null
[2.02GiB/s] # <-- cat does 128K writes onto default 64K sized FIFO pipe (???)

# from char devices

$ timeout 5 rat </dev/zero | pv -ab >/dev/null
25.2GiB [5.05GiB/s]
$ timeout 5 cat </dev/zero | pv -ab >/dev/null
16.0GiB [4.00GiB/s]

# between pipes

$ timeout -s SIGINT 5 yes | rat | pv -r >/dev/null
[4.68GiB/s]
$ timeout -s SIGINT 5 yes | cat | pv -r >/dev/null
[2.80GiB/s]
```

#### Argument ordering, error logging, sanity checks
```
$ echo test | rat - /does/not/exists /etc/hosts /does/not/exists2 | md5sum
rat: /does/not/exists: No such file or directory
rat: /does/not/exists2: No such file or directory
27f2e6689a97a42813e55d44ef29cda4  -
$ rat < foo >> foo
rat: -: input file is output file

$ echo test | cat - /does/not/exists /etc/hosts /does/not/exists2 | md5sum
cat: /does/not/exists: No such file or directory
cat: /does/not/exists2: No such file or directory
27f2e6689a97a42813e55d44ef29cda4  -
$ cat < foo >> foo
cat: -: input file is output file
```

#### Some comparisons with `pv` and `uu-cat`
```
$ timeout 5 pv -r </dev/zero >/dev/null
[20.3GiB/s]
$ timeout 5 yes | pv -r >/dev/null
[6.13GiB/s]
$ timeout 5 pv -r </dev/zero | pv -q >/dev/null
[3.39GiB/s]
$ timeout 5 pv -r </dev/zero | uu-cat >/dev/null
[3.66GiB/s]
$ timeout 5 pv -r </dev/zero | rat >/dev/null
[3.26GiB/s]
$ timeout 5 pv -r </dev/zero | pv -q --no-splice >/dev/null
[2.70GiB/s]
$ timeout 5 pv -r </dev/zero | cat >/dev/null
[2.66GiB/s]
```

#### Increases throughput by configuring pipe sizes
```
$ timeout 5 cat </dev/zero | pv -abC >/dev/null
10.8GiB [2.71GiB/s]
$ timeout 5 cat </dev/zero | rat | pv -abC >/dev/null # without splice
17.7GiB [3.54GiB/s]
$ timeout 5 cat </dev/zero | rat | pv -ab >/dev/null  # with splice
19.4GiB [3.88GiB/s]
```

#### Splice it up!
```
$ timeout 5 rat </dev/zero | rat | rat | rat | pv -r >/dev/null
[4.78GiB/s]

$ timeout 5 rat </dev/zero | cat | cat | cat | pv -r >/dev/null
[2.16GiB/s]
```

### Motivation

I just wanted to do this as a learning experience for rust.

At least, that's how it started.

I intend to make `rat` nearly the same as `cat` (uutils already did all this) but with additional niceties built in, maybe such as:

- Prefixing lines with timestamps in any arbitrary strftime format
- Strict mode - pre-emptively detect errors ie. missing files / permissions before providing possibly mangled output
- Human readable, colorized output of any generic text stream based on patterns (ie. red errors, blue debugs, etc)

### Thoughts / Notes

- `Stdout` in rust will always be wrapped by `LineWriter` which flushes the buffer on [new lines][6]. This seems fine for interactive stdin.

  For other I/O use the wrapped `BufWriter<File>` on the file descriptor for more flow control otherwise you get a ton of unnescessary small writes.

- Pre-allocation given a Sized `Vec<u8>` for the buffer handles has some interesting impacts on runtime performance

  Even when that vector is immediately cleared on runtime the behavior between _initially empty_ vs. _initially padded_ (_Sized_?) vector is noticeable.
  `read()` calls seem to ramp up by pow2 starting at 8192 until it reaches the specified buffer size, instead of just passing the fixed amount of data.

  Alternatively `Vec.with_capacity` works too and apparently doesn't need to be cleared, so one less line of code.

- `splice(2)` can show some insane performance improvements over traditional read()/write() calls

  However, these benefits are only fully realized under specifics conditions (ie. `</dev/zero >/dev/null`) which don't apply to writes on regular files.
  There is still an improvement over traditional syscalls.
  Probably excellent for [network sockets...](https://blog.superpat.com/zero-copy-in-linux-with-sendfile-and-splice)

- Linux pipes are limited to 64K buffers by default. They can be increased up to the sysctl `fs.pipe-max-size` setting (1MB by default).

  You can tweak pipes using `fcntl()` - see [pipe(7) - Pipe Capacity](https://man7.org/linux/man-pages/man7/pipe.7.html) and [fcntl(2) - Changing the capacity of a pipe](https://man7.org/linux/man-pages/man2/fcntl.2.html).

- GNU `cat` has odd behavior when writing to pipes, it clearly attempts to write its default 128K buffer size which subsequently reduces the performance.

  Only happens when on the left-side of the pipe (writing), reading pipes will fill and flush the 64K buffer immediately as expected. In between pipes (ie. `echo | cat | grep -`) will perform 64K read and write.

  `rat` easily acheives ~500MB-1GBps+ more throughput here by using the proper pipe buffer size (see above)

- rust `io::copy` currently insists on using `sendfile()` first and then using `copy_file_range()` on subsequent calls during the same runtime (ie. when given multiple parameters), also the `copy_file_range()` length is way lower than `cat` for example (`1073741824` vs `9223372035781033984`)
  
  Reported and fixed: https://github.com/rust-lang/rust/issues/114341
  
- How can `copy_file_range()` concatenate a file multiple times (ie. each syscall is appending to the file) and yet doesn't work (EBADF) when appending from shell?

### Upstream bug fixes

- https://github.com/rust-lang/rust/pull/114373

### Known Bugs

See `TODO` in [`rat.rs`](/src/bin/rat.rs)

[//]: # (References)
[1]: https://news.ycombinator.com/item?id=31592934
[2]: https://old.reddit.com/r/unix/comments/6gxduc/how_is_gnu_yes_so_fast/
[3]: https://github.com/coreutils/coreutils/blob/master/src/cat.c
[4]: https://github.com/coreutils/coreutils/blob/master/src/ioblksize.h#L77
[5]: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/linux/pipe_fs_i.h
[6]: https://github.com/rust-lang/libs-team/issues/148
