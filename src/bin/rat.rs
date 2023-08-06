/*
 * References:
 * https://old.reddit.com/r/unix/comments/6gxduc/how_is_gnu_yes_so_fast/
 * https://github.com/coreutils/coreutils/blob/master/src/cat.c
 *
 * high-level breakdown:
 *
 * parse arguments
 * determine output characteristics (fstat/etc)
 * -> io_blksize?
 * if not formatted set O_BINARY file mode for stdout
 * start main loop, iterating over cli args, start by checking if input stdin ("-")
 * -> if files are provided as args, iterate on those (stdin is completely ignored)
 * --> if not formatted set O_BINARY file mode for stdin/file
 * ---> if error then report, continue
 * --> get fstat of the input descriptor
 * ----> io_blksize? https://github.com/coreutils/coreutils/blob/master/src/ioblksize.h#L77
 * --> FADVISE_SEQUENTIAL if regular file (after file open)
 * --> error if _non empty_ input file (regular) == output file, continue
 * --> determine implementation (copy_cat / simple_cat / cat)
 * ---> `copy_cat` if no output formatting options provided and input/output are regular files both on the same device (?)
 * ---> `simple_cat` if no output formatting options and `copy_cat` fails because irregular files (stdin/stdout) or different devices (?)
 * ---> `cat` if output is formatted
 * --> chain results using bitwise AND operator (&=) on boolean (`ok`) to determine final exit code
 * --> any failures results in non-zero exit code regardless of where it occurred in the loop
 * -> continue loop if more files (arguments) remain
 *
 *
 * Interactive usage feature ideas (to distinguish `rat` from `cat`):
 * - timestamp each output line (-t / --timestamp)
 * - automatically re-pipe / re-direct through `pv` (--pv)
 * --> and `jq` / `yq` (re-implement those directly?)
 * --> line by line - buffering output lines in reverse, waiting until user hits enter
 * --> color automatically by detected line type (ie. `error`, `warn`, `info`, etc)
 * --> support `cut` like behavior on each line directly to keep ie. timestamps, color, etc
 * --> strict mode, do not gracefully ignore missing files like cat (ie. cat /does/not/exist /etc/hosts)
 * --> escape characters mode (escape_default?)
 */

use clap::Parser;
use nix::fcntl;
use nix::fcntl::PosixFadviseAdvice::POSIX_FADV_SEQUENTIAL;
use std::cmp::min;
use std::fs::{File, Metadata};
use std::io::{self, BufRead, BufReader, BufWriter, ErrorKind, IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::os::linux::fs::MetadataExt;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::AsRawFd;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)] // Read from `Cargo.toml`
#[command(next_line_help = true)]
struct Cli {
    /// Use custom stack copy only (read/write syscalls)
    #[clap(long, action)]
    no_iocopy: bool,
    /// Do not gracefully allow errors
    #[clap(long, short, action)]
    strict: bool,
    /// Unbuffered character writes (implies --no-iocopy)
    #[clap(long, short, action)]
    unbuffered: bool,
    /// Optional file paths to read, stdin by default
    paths: Option<Vec<String>>,
}

// using i32 here since `fcntl::F_GETPIPE_SZ` calls returns the same
const IO_BUFSIZE: i32 = 1 << 17; // or 2^17 or 131072 (bytes) or 32 pages (4K each usually)
const NEWLINE_CH: u8 = 10; // 0x0A

fn is_same_file(imeta: &Metadata, ometa: &Metadata) -> bool {
    imeta.is_file()
        && ometa.is_file() // I AM THE OMETA
        && imeta.st_dev() == ometa.st_dev()
        && imeta.st_ino() == ometa.st_ino()
        && imeta.st_size() != 0
}

/*
 * Stdout/StdoutLock is wrapped by LineWriter which always flushes writes on newline char:
 * https://doc.rust-lang.org/std/io/struct.LineWriter.html
 * https://github.com/rust-lang/rust/blob/8771282d4e7a5c4569e49d1f878fb3ba90a974d0/library/std/src/io/stdio.rs#L535
 * https://github.com/rust-lang/rust/blob/8771282d4e7a5c4569e49d1f878fb3ba90a974d0/library/std/src/io/buffered/linewriter.rs
 *
 * Instead, use BufWriter on the raw file descriptor (std::io::StdoutRaw is not exposed)
 * This seems like a common complaint:
 * https://github.com/rust-lang/rust/issues/58326
 * https://github.com/rust-lang/libs-team/issues/148
 */
fn simple_rat<R: Read, W: Write>(
    args: &Cli,
    input: &mut BufReader<R>,
    output: &mut BufWriter<W>,
    is_tty: bool,
) -> io::Result<u64> {
    // Fully buffered output by default
    let mut _bufch: u8 = 0;
    let unbuffered = args.unbuffered;

    let ibufsize: u64 = input.capacity().try_into().unwrap();
    let mut read = |buffer: &mut Vec<u8>, bufch: u8| -> io::Result<usize> {
        let mut input = input.take(ibufsize);
        // ie. read up until newline when interactive
        // TODO: unbuffered reads?
        if bufch > 0 {
            return input.read_until(bufch, buffer);
        };
        input.read_to_end(buffer)
    };

    let mut write = |buffer: &mut Vec<u8>| -> io::Result<()> {
        // TODO: how to prepend output ie. timestamps etc:
        // Insert generic functions here for arbitrary formatting?
        //let _prefix = "[TEST] ".as_bytes();
        //output.write(_prefix)?;
        if unbuffered {
            for c in buffer.drain(..) {
                output.write(&[c])?;
                output.flush()?;
            }
        }
        output.write_all(buffer.drain(..).as_ref())?;
        output.flush() // Noop unless we're line buffering?
    };

    if is_tty {
        // or format
        _bufch = NEWLINE_CH;
    } else if !args.no_iocopy {
        // copy_cat equivalent, plus some `splice(2)` goodness for inter-pipe
        // In rust 1.73 this bug will be fixed: https://github.com/rust-lang/rust/pull/114373
        return io::copy(input, output);
    }

    // Fallback to custom IO loop for formatting/etc
    let mut buffer = Vec::with_capacity(ibufsize as usize);
    loop {
        match read(&mut buffer, _bufch) {
            // EOF
            Ok(0) => break,
            // Data in the buffer
            Ok(..) => write(&mut buffer)?,
            // Raise errors
            Err(e) => return Err(e),
        }
    }
    // We don't care about the result here, just needed for `return io::copy` above
    Ok(0)
}

fn cli(ok: &mut bool, mut args: Cli) -> io::Result<()> {
    // lock these standard file descriptors, they are subsequently F_DUPFD_CLOEXEC
    // hence fd=0 -> fd=3 and fd=1 -> fd=4 (this is not a problem)
    // stdio might be re-used throughout the runtime, let's just reference it
    let ref stdin = File::from(io::stdin().lock().as_fd().try_clone_to_owned()?);
    let ref stdout = File::from(io::stdout().lock().as_fd().try_clone_to_owned()?);

    let mut obufsize = IO_BUFSIZE;
    let _stdout_meta = stdout.metadata()?;
    if _stdout_meta.file_type().is_fifo() {
        // Increasing pipes from the default 64K size doesn't seem to actually help
        //fcntl::fcntl(stdout.as_raw_fd(), fcntl::F_SETPIPE_SZ(IO_BUFSIZE))?;
        obufsize = fcntl::fcntl(stdout.as_raw_fd(), fcntl::F_GETPIPE_SZ)?;
    }

    if args.unbuffered {
        args.no_iocopy = true;
    }

    // Is there a way to use the clap derive for default here?
    let paths = args
        .paths
        .clone()
        .unwrap_or_else(|| vec![String::from("-")]);

    for file in paths {
        let mut is_tty = stdout.is_terminal(); // false here allows io::copy to sendfile to interactive stdout (!?)
        let mut is_stdin = false;
        let mut ibufsize = IO_BUFSIZE;
        let mut filename = file.as_str();
        // You could also pass the stdin character devices explicitly...
        if file == "-" || file == "/dev/stdin" || file == "/proc/self/fd/0" {
            // We want to keep the actual argument value for error message compatibility
            // But we need to get metadata via one of the symlinks (ie. we can't from `-`)
            filename = "/dev/stdin";
            is_stdin = true;
        }

        // We need handle to be consistent Ok(&File) to match stdin - how else could we do this DRYly?
        // maybe passing `dyn` type or boxing or some other generic-ism?
        let handle: io::Result<_>;
        let _fhandle: File;
        if is_stdin {
            is_tty |= stdin.is_terminal();
            handle = Ok(stdin);
        } else {
            let _result = File::open(filename);
            if let Err(e) = _result {
                *ok &= false;
                //eprintln!("{:#?}", e);
                match e.kind() {
                    // Also
                    // rat: $#t: No such file or directory
                    // cat: '$#t': No such file or directory
                    ErrorKind::NotFound => eprintln!("rat: {file}: No such file or directory"),
                    ErrorKind::PermissionDenied => eprintln!("rat: {file}: Permission denied"),
                    _ => todo!(),
                };
                continue;
            }
            _fhandle = _result.unwrap();
            handle = Ok(&_fhandle)
        }

        match handle {
            Ok(input) => {
                // cat also does this regardless of input type, discards any errors, ie. ESPIPE
                let _ = nix::fcntl::posix_fadvise(input.as_raw_fd(), 0, 0, POSIX_FADV_SEQUENTIAL);
                if let Ok(_input_meta) = input.metadata() {
                    if is_same_file(&_input_meta, &_stdout_meta) {
                        *ok &= false;
                        eprintln!("rat: {file}: input file is output file");
                        continue;
                    }
                    if _input_meta.file_type().is_fifo() {
                        ibufsize = fcntl::fcntl(input.as_raw_fd(), fcntl::F_GETPIPE_SZ)?;
                    }
                }
                // Decoupling the buffer sizes causes massive performance hit with pipes
                ibufsize = min(ibufsize, obufsize);
                simple_rat(
                    &args,
                    // cat uses a single shared buffer to read into and write from
                    // that doesn't seem possible in rust using safe interfaces (??)
                    // So, ultimately we have 3 buffers: 1 in, 1 out, 1 to move data between
                    // io::copy does some black magic using unstable BorrowedBuf
                    // https://doc.rust-lang.org/src/std/io/copy.rs.html#137-163
                    BufReader::with_capacity(ibufsize as usize, input).by_ref(),
                    BufWriter::with_capacity(obufsize as usize, stdout).by_ref(),
                    is_tty,
                )
                .unwrap_or_else(|e| {
                    // TODO: this catches trying to read directories/etc
                    // slightly different than cat like this:
                    // cat: t: Is a directory
                    // rat: t: Is a directory (os error 21)
                    eprintln!("rat: {file}: {}", e);
                    42u64 // Why not?
                });
            }
            Err(_) => { /* We preempt this above */ }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    // `ok` tracks if any file errors occurred in the loop
    let mut ok = true;
    cli(&mut ok, Cli::parse()).unwrap_or_else(|e| {
        eprintln!("{:#?}", e);
    });
    match ok {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    }
}
