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

use nix::fcntl;
use std::cmp::min;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, ErrorKind, Read, Write};
use std::os::linux::fs::MetadataExt;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::FromRawFd; //AsRawFd
use std::process::ExitCode;

const IO_BUFSIZE: i32 = 1 << 17; // or 2^17 or 131072 (bytes) or 32 pages (4K each usually)
const NEWLINE_CH: u8 = 10; // 0x0A
const STDIN_FD: i32 = 0;
const STDOUT_FD: i32 = 1;

extern "C" fn get_pagesize() -> i64 {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) }
}

extern "C" fn isatty(fd: i32) -> bool {
    unsafe { libc::isatty(fd) != 0 }
}

/*
 * Stdout/StdoutLock is wrapped by LineWriter which always flushes writes on newline char:
 * https://doc.rust-lang.org/std/io/struct.LineWriter.html
 * https://github.com/rust-lang/rust/blob/8771282d4e7a5c4569e49d1f878fb3ba90a974d0/library/std/src/io/stdio.rs#L535
 * https://github.com/rust-lang/rust/blob/8771282d4e7a5c4569e49d1f878fb3ba90a974d0/library/std/src/io/buffered/linewriter.rs
 *
 * Instead, use BufWriter<File> on the raw file descriptor (std::io::StdoutRaw is not exposed)
 * This seems like a common complaint:
 * https://github.com/rust-lang/rust/issues/58326
 * https://github.com/rust-lang/libs-team/issues/148
 */
fn simple_cat(
    input: &mut BufReader<File>,
    output: &mut BufWriter<File>,
    bufsize: u64,
    is_stdin: bool,
) -> io::Result<()> {
    // Vec<u8> holds binary UTF-8 characters, no concept of "text mode" here
    let mut buffer: Vec<u8> = Vec::with_capacity(bufsize as usize);
    // Use `take` and `read_to_end` to limit reads given the desired bufsize
    // Alternatively provide `bufch` to break on a specific byte char
    let mut read = |buffer: &mut Vec<u8>, bufch: u8| -> io::Result<usize> {
        let mut input = input.take(bufsize);
        // ie. read up until newline when interactive
        if bufch > 0 {
            return input.read_until(bufch, buffer);
        };
        input.read_to_end(buffer)
    };

    let mut write = |buffer: &mut Vec<u8>| -> io::Result<()> {
        // TODO: how to prepend output ie. timestamps etc:
        // Insert generic functions here for arbitrary formatting?
        //let _buffer = ["[TEST] ".as_bytes(), buffer.as_slice()].concat();
        //*buffer = _buffer;
        //output.write(buffer)?;
        // totally unbuffered (character) write mode:
        //for i in 0..buffer.len() {
        //    output.write(&buffer[i..i+1])?;
        //    output.flush()?;
        //}
        output.write_all(buffer.drain(..).as_ref())
    };

    // Fully buffered output by default
    let mut bufch: u8 = 0;
    // Line buffered when we are fairly certain of interactive terminal
    // Will also be needed for formatted output modes
    let is_stdin_tty = is_stdin && isatty(STDIN_FD);
    let is_stdout_tty = isatty(STDOUT_FD);
    // cat doesn't care about stdout
    if is_stdin_tty || is_stdout_tty {
        bufch = NEWLINE_CH;
    }
    loop {
        match read(&mut buffer, bufch) {
            // EOF
            Ok(0) => break,
            // Data in the buffer
            Ok(..) => write(&mut buffer)?,
            // Raise errors
            Err(e) => return Err(e),
        }
    }
    // Just for sanity, also implicitly returns Ok(()) after loop break above
    output.flush()
}

fn cli(ok: &mut bool, strict: bool) -> std::io::Result<()> {
    // Get the common bufsize for IO given the specific inputs and outputs
    // ie. regular files should r/w at the default 128KB buffer sizes
    // fifo pipes should r/w at the max 64KB buffer size on Linux
    // GNU cat notably attempts to write 128KB into fifo pipes and is slower
    // This is assuming read()/write() calls with no splice magic (like `uu-cat`)
    // https://man7.org/linux/man-pages/man2/splice.2.html
    let page_size = get_pagesize();
    let minbufsize = |i: i32, o: i32| -> u64 {
        let _min = min(i, o) as i64;
        if _min % page_size != 0 {
            // Just for sanity, shouldn't ever happen?
            panic!(
                "minimum buffer is not aligned to page size: {} % {} != 0",
                _min, page_size
            )
        }
        _min as u64
    };

    // Not sure these locks are necessary in our use case here
    // Makes no difference
    let (_, _) = (io::stdin().lock(), io::stdout().lock());
    // Determine output characteristics, follows the /dev/stdout symlink
    let mut obufsize = IO_BUFSIZE;
    let _stdout_stat = fs::metadata("/dev/stdout")?;
    if _stdout_stat.file_type().is_fifo() {
        //obufsize = fcntl::fcntl(STDOUT_FD, fcntl::F_SETPIPE_SZ(IO_BUFSIZE))?;
        obufsize = fcntl::fcntl(STDOUT_FD, fcntl::F_GETPIPE_SZ)?;
    }

    // Skip the 0th cmdline arg here and re-collect as Strings
    let mut args: Vec<String> = env::args().skip(1).collect();
    // Pass implict stdin if not given any parameters
    if args.len() == 0 {
        args.push(String::from("-"));
    }

    // stdout is re-used
    let mut stdout = BufWriter::new(unsafe { File::from_raw_fd(STDOUT_FD) });
    // Only accept positional file arguments for now
    for file in args {
        let mut ibufsize = IO_BUFSIZE;
        let mut _file = file.as_str();
        // You could also pass the stdin character devices explicitly...
        if file == "-" || file == "/dev/stdin" || file == "/proc/self/fd/0" {
            // We want to keep the actual argument value for error message compatibility
            // But we need to get metadata via one of the symlinks (ie. we can't from `-`)
            _file = "/dev/stdin";
        }
        let is_stdin = _file == "/dev/stdin";

        let mut _both_reg: bool = false;
        let mut _appending: bool = false;
        // Use `if let` here to ignore ENOENT error on non-existing files, etc
        if let Ok(_file_stat) = fs::metadata(_file) {
            if _file_stat.is_file()
                && _stdout_stat.is_file()
                && _file_stat.st_dev() == _stdout_stat.st_dev()
                && _file_stat.st_ino() == _stdout_stat.st_ino()
                && _file_stat.st_size() != 0
            {
                /*
                 * Same as coreutils cat.
                 *
                 * echo hello > foo
                 * rat foo >> foo
                 *
                 * Note the append (>>), when overwriting (>) the shell completely truncates the file
                 * which results in a empty file regardless of whatever command is run. (ie. see `>foo` or `:>foo`)
                 */
                eprintln!("rat: {}: input file is output file", file);
                *ok &= false;
                continue;
            }
            if _file_stat.file_type().is_fifo() {
                //ibufsize = fcntl::fcntl(STDIN_FD, fcntl::F_SETPIPE_SZ(IO_BUFSIZE))?;
                ibufsize = fcntl::fcntl(STDIN_FD, fcntl::F_GETPIPE_SZ)?;
            }
            // If both files are regular we can attempt faster copy method
            _both_reg = _file_stat.is_file() && _stdout_stat.is_file();
            // Destination hasn't been truncated, we're appending
            _appending = _stdout_stat.st_size() > 0;
        }

        // TODO: fadvise(POSIX_FADV_SEQUENTIAL) ?
        // Why is it so difficult to manage these file descriptors in rust?
        let _open_file = || {
            if is_stdin {
                return Ok(unsafe { File::from_raw_fd(STDIN_FD) });
            };
            File::open(&file)
        };

        match _open_file() {
            Ok(mut f) => {
                // Attempt copy_cat equivalent
                // If we detect that we're appending, skip this, to use bigger buffers
                if _both_reg && !_appending {
                    // TODO: io::copy seems to use sendfile() first then copy_file_range()
                    // this also falls back to 8192 buffers for read/write ie. if appending
                    if let Ok(_) = io::copy(&mut f, &mut stdout) {
                        continue;
                    }
                };
                simple_cat(
                    BufReader::new(f).by_ref(),
                    &mut stdout,
                    minbufsize(ibufsize, obufsize),
                    is_stdin,
                )?
            }
            Err(e) => {
                *ok &= false;
                //eprintln!("{:#?}", e);
                match e.kind() {
                    ErrorKind::NotFound => eprintln!("rat: {}: No such file or directory", &file),
                    ErrorKind::PermissionDenied => eprintln!("rat: {}: Permission denied", &file),
                    _ => todo!(),
                };
                if strict {
                    // This needs to happen earlier.
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    // `ok` tracks if any file errors occurred in the loop
    let mut ok = true;
    let _ = cli(&mut ok, false);
    match ok {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    }
}
