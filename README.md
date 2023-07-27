# ratiscat

`rat` is a performance _matching_ re-implementation of `cat` in rust

Turns out a rat fits in a pipe better than a cat anyways.

![Baba is You Cat is Jelly](img/baba_cat_is_jelly.png)

See [`rat.rs`](/src/bin/rat.rs)

Maintaining the `read()` / `write()` syscalls pattern via rust abstractions, with a few improvements.

For much higher performance check out the usage of splice in `uu-cat` (comparison below).

https://github.com/uutils/coreutils/

### Usage

As run on and compared to: i7-6800K / 128GB DDR4 (file reads cached) / Linux 6.3.9 / coreutils 9.3 / uutils 0.0.20

#### Regular file -> regular file (uses `copy_file_range` if possible)

```
# 4GB random data sample
$ dd if=/dev/urandom of=test.rand bs=1MB count=4096
$ time rat test.rand >test.$(date +%s)
real    0m0.004s
user    0m0.004s
sys     0m0.000s
$ time cat test.rand >test.$(date +%s)
real    0m0.004s
user    0m0.004s
sys     0m0.000s
```

#### Regular file or character device -> Pipe (FIFO)
```
$ rat test.rand | pv -r >/dev/null
[2.69GiB/s] # <-- rat does 64K R/W on FIFO pipes, smaller buffers are better sometimes
$ cat test.rand | pv -r >/dev/null
[2.02GiB/s] # <-- cat does 128K writes onto FIFO pipe but 64K reads (??)

$ timeout 5 rat /dev/zero | pv -r >/dev/null
[5.12GiB/s]
$ timeout 5 cat /dev/zero | pv -r >/dev/null
[4.21GiB/s]
```

#### Pipe <-> Pipe
```
$ timeout -s SIGINT 5 yes | rat | pv -r >/dev/null
[2.85GiB/s]
$ timeout -s SIGINT 5 yes | cat | pv -r >/dev/null
[2.86GiB/s]
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

#### Some comparisons with `pv` and `uu-cat` (which use splice - try `pv --no-splice`)
```
$ timeout 5 pv -r </dev/zero >/dev/null
[20.3GiB/s]
$ timeout 5 yes | pv -r >/dev/null
[6.13GiB/s]
$ timeout 5 pv -r </dev/zero | pv -q >/dev/null
[3.39GiB/s]
$ timeout 5 pv -r </dev/zero | uu-cat >/dev/null
[3.66GiB/s]
$ timeout 5 pv -r </dev/zero | pv -q --no-splice >/dev/null
[2.70GiB/s]
$ timeout 5 pv -r </dev/zero | rat >/dev/null
[2.71GiB/s]
$ timeout 5 pv -r </dev/zero | cat >/dev/null
[2.66GiB/s]
```

### Motivation

I just wanted to do this as a learning experience for rust.

At least, that's how it started.

I intend to make `rat` nearly the same as `cat` (uutils already did all this) but with additional niceties built in, maybe such as:

- Prefixing lines with timestamps in any arbitrary strftime format
- Strict mode - pre-emptively detect errors ie. missing files / permissions before providing possibly mangled output
- Human readable, colorized output of any generic text stream based on patterns (ie. red errors, blue debugs, etc)

Things learned so far in general:

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

  Only happens when on the left-side of the pipe (writing), reading pipes will fill and flush the 64K buffer immediately as expected. In between pipes (ie. `echo | cat | grep`) will perform 64K read and write.

  `rat` easily acheives ~500MB-1GBps+ more throughput here by using the proper pipe buffer size (see above)

### Known Bugs

See `TODO` in [`rat.rs`](/src/bin/rat.rs)

- Ctrl+D needs to be pressed twice while interactive

[//]: # (References)
[1]: https://news.ycombinator.com/item?id=31592934
[2]: https://old.reddit.com/r/unix/comments/6gxduc/how_is_gnu_yes_so_fast/
[3]: https://github.com/coreutils/coreutils/blob/master/src/cat.c
[4]: https://github.com/coreutils/coreutils/blob/master/src/ioblksize.h#L77
[5]: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/linux/pipe_fs_i.h
[6]: https://github.com/rust-lang/libs-team/issues/148
