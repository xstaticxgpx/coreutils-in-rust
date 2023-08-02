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
use std::io::{self, BufRead, BufReader, BufWriter, ErrorKind, Read, Write};
use std::os::fd::AsFd; //FromRawFd
use std::os::linux::fs::MetadataExt;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::AsRawFd;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)] // Read from `Cargo.toml`
#[command(next_line_help = true)]
struct Cli {
    /// Optional file paths to read, stdin by default
    paths: Option<Vec<String>>,
}

// using i32 here since `fcntl::F_GETPIPE_SZ` calls returns the same
const IO_BUFSIZE: i32 = 1 << 17; // or 2^17 or 131072 (bytes) or 32 pages (4K each usually)
const NEWLINE_CH: u8 = 10; // 0x0A
const STDIN_FD: i32 = 0;
//const STDOUT_FD: i32 = 1;

// TODO: use IsTerminal or something
extern "C" fn isatty(fd: i32) -> bool {
    unsafe { libc::isatty(fd) != 0 }
}

fn is_same_file(imeta: Metadata, ometa: &Metadata) -> bool {
    if imeta.is_file()
        && ometa.is_file()
        && imeta.st_dev() == ometa.st_dev()
        && imeta.st_ino() == ometa.st_ino()
        && imeta.st_size() != 0
    {
        /*
         * Same as coreutils cat.
         *
         * echo hello > foo
         * rat foo >> foo
         *
         * Note the append (>>), when overwriting (>) the shell preemptively truncates the file.
         */
        return true;
    }
    false
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
fn simple_rat(
    input: &mut BufReader<File>,
    output: &mut BufWriter<&File>,
    is_tty: bool,
) -> io::Result<u64> {
    // Fully buffered output by default
    let mut _bufch: u8 = 0;

    let cap: u64 = input.capacity().try_into().unwrap();
    let mut read = |buffer: &mut Vec<u8>, bufch: u8| -> io::Result<usize> {
        let mut input = input.take(cap);
        // ie. read up until newline when interactive
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
        // TODO: totally unbuffered (character) write mode:
        //for i in 0..buffer.len() {
        //    output.write(&buffer[i..i+1])?;
        //    output.flush()?;
        //}
        output.write_all(buffer.drain(..).as_ref())?;
        output.flush() // Noop unless we're line buffering?
    };

    if is_tty {
        // or format
        _bufch = NEWLINE_CH;
    } else {
        // copy_cat equivalent, plus some `splice(2)` goodness for inter-pipe
        return io::copy(input, output);
    }

    // Fallback to custom IO loop for formatting/etc
    let mut buffer = Vec::with_capacity(IO_BUFSIZE as usize);
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

fn cli(ok: &mut bool, args: Cli) -> io::Result<()> {
    //println!("{args:#?}");
    // lock these standard file descriptors, later they are F_DUPFD_CLOEXEC during clone_to_owned
    // hence fd=1 -> fd=3 below and maybe later fd=0 -> fd=4 (this is not a problem)
    let (_stdin, _stdout) = (io::stdin().lock(), io::stdout().lock());

    // stdout is re-used throughout the runtime, let's just reference it
    let ref stdout = File::from(_stdout.as_fd().try_clone_to_owned()?);

    // Determine output characteristics, follows the /dev/stdout symlink
    let mut obufsize = IO_BUFSIZE;
    let _stdout_stat = stdout.metadata()?;
    if _stdout_stat.file_type().is_fifo() {
        // Increasing pipes from the default 64K size doesn't seem to actually help
        //fcntl::fcntl(stdout.as_raw_fd(), fcntl::F_SETPIPE_SZ(IO_BUFSIZE))?;
        obufsize = fcntl::fcntl(stdout.as_raw_fd(), fcntl::F_GETPIPE_SZ)?;
    }

    // Is there a way to use the clap derive for default here?
    let paths: Vec<_> = args.paths.unwrap_or_else(|| vec![String::from("-")]);

    // Only accept positional file arguments for now
    for file in paths {
        let handle: io::Result<_>;
        let mut is_tty = false;
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

        // We need handle to be consistent File - how else could we do this DRYly?
        // maybe passing `dyn` type to the BufReader or some other generic-ism?
        if is_stdin {
            handle = Ok(File::from(_stdin.as_fd().try_clone_to_owned()?));
            is_tty = isatty(STDIN_FD);
        } else {
            handle = File::open(filename);
        }

        match handle {
            Ok(input) => {
                // cat also does this regardless of input type, discards any errors, ie. ESPIPE
                let _ = nix::fcntl::posix_fadvise(input.as_raw_fd(), 0, 0, POSIX_FADV_SEQUENTIAL);
                if is_same_file(input.metadata()?, &_stdout_stat) {
                    *ok &= false;
                    eprintln!("rat: {file}: input file is output file");
                    continue;
                }
                // Decoupling the buffer sizes causes massive performance hit
                ibufsize = min(ibufsize, obufsize);
                simple_rat(
                    // cat uses a single shared buffer to read into and write from
                    // that doesn't seem possible in rust using safe interfaces (??)
                    // So, ultimately we have 3 buffers: 1 in, 1 out, 1 to move data between
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
            Err(e) => {
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
            }
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
