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
 */

use std::cmp::min;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::os::linux::fs::MetadataExt;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::FromRawFd; //AsRawFd
use std::process::ExitCode;

// https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/linux/pipe_fs_i.h
// s/PIPE_DEF_BUFFERS/PIPE_DEF_PAGES/ for clarity here
// TODO: can we get this from the system somehow?
const PIPE_DEF_PAGES: u64 = 16;

const IO_BUFSIZE: u64 = 1 << 17; // or 2^17 or 131072 (bytes) or 32 pages (4K each usually)
const NEWLINE_CH: u8 = 10; // 0x0A
const STDIN_FD: i32 = 0;
const STDOUT_FD: i32 = 1;

extern "C" fn get_pagesize() -> u64 {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 }
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
) -> io::Result<()> {
    // Determines if we should flush on newline (interactive terminal)
    let is_stdin_tty = isatty(STDIN_FD);
    let is_stdout_tty = isatty(STDOUT_FD);
    // Vec<u8> holds binary UTF-8 characters, no concept of "text mode" here
    let mut buffer: Vec<u8> = Vec::with_capacity(bufsize as usize);
    // Use `take` and `read_to_end` to limit reads given the desired bufsize
    #[rustfmt::skip]
    let mut read =  |buffer: &mut Vec<u8>| -> io::Result<usize> {
        let mut _in = input.take(bufsize);
        if is_stdin_tty && is_stdout_tty {
            // read up until newline when interactive
            // TODO: totally unbuffered (character) mode?
            return _in.read_until(NEWLINE_CH, buffer)
        };
        _in.read_to_end(buffer)
    };
    // Use `write_all` here to ensure we aren't dropping any output
    let mut write = |buffer: &mut Vec<u8>| -> io::Result<()> {
        output.write_all(buffer)?;
        if is_stdout_tty {
            // output is a terminal and we've reached a newline
            output.flush()?;
        }
        buffer.clear();
        Ok(())
    };
    loop {
        eprintln!("loop");
        match read(&mut buffer) {
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
    // Define two different buffers, since input/output characteristics could differ
    // and require a lower common-denominator bufsize for performance (ie. pipes)
    let (mut ibufsize, mut obufsize) = (IO_BUFSIZE, IO_BUFSIZE);

    // Get the common bufsize for IO given the specific inputs and outputs
    // ie. regular files should r/w at the default 128KB buffer sizes
    // fifo pipes should r/w at the max 64KB buffer size on Linux
    // GNU cat notably attempts to write 128KB into fifo pipes and is slower
    // This is assuming read()/write() calls with no splice magic (like `uu-cat`)
    // https://man7.org/linux/man-pages/man2/splice.2.html
    let page_size = get_pagesize();
    let minbufsize = |i, o| {
        let _min = min(i, o);
        if _min % page_size != 0 {
            // Just for sanity, shouldn't ever happen?
            panic!(
                "minimum buffer is not aligned to page size: {} % {} != 0",
                _min, page_size
            )
        }
        _min
    };

    // Not sure these locks are necessary in our use case here
    // Makes no difference
    let (_, _) = (io::stdin().lock(), io::stdout().lock());
    // We're re-opening stdin below to reuse existing logic
    // stdin can only be consumed once, stdout is shared
    let mut stdout = BufWriter::new(unsafe { File::from_raw_fd(STDOUT_FD) });

    // Determine output characteristics, follows the /dev/stdout symlink
    let _stdout_stat = fs::metadata("/dev/stdout")?;
    if _stdout_stat.file_type().is_fifo() {
        obufsize = PIPE_DEF_PAGES * page_size;
    }

    // Skip the 0th cmdline arg here and re-collect as Strings
    let mut args: Vec<String> = env::args().skip(1).collect();
    // Pass implict stdin if not given any parameters
    if args.len() == 0 {
        args.push(String::from("/dev/stdin"));
    }

    // Only accept positional file arguments for now
    for mut file in args {
        // You could also pass the stdin character devices explicitly...
        if file == "-" || file == "/dev/stdin" || file == "/proc/self/fd/0" {
            // Let's just use /dev/stdin across the board (for symlink follow metadata)
            file = String::from("/dev/stdin");
        }

        // Use `if let` here to ignore ENOENT error on non-existing files, etc
        // TODO: strict mode?
        let get_file_stat = || fs::metadata(&file);
        if let Ok(_file_stat) = get_file_stat() {
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
                let mut _compat = |file| {
                    if file == "/dev/stdin" {
                        // $ ./rat < foo >> foo
                        // rat: -: input file is output file
                        return String::from("-");
                    }
                    file
                };
                eprintln!("rat: {}: input file is output file", _compat(file));
                continue;
            }
            if _file_stat.file_type().is_fifo() {
                ibufsize = PIPE_DEF_PAGES * page_size;
            }
        }

        // TODO: this will unnescessarily re-open /dev/stdin as fd=3 (no impact)
        // TODO: fadvise(POSIX_FADV_SEQUENTIAL) ?
        match File::open(&file) {
            Ok(f) => {
                // Attempt copy_cat equivalent (uses `copy_file_range`)
                // /dev/stdout can be a symlink to our output
                if let Ok(_) = fs::copy(file, "/dev/stdout") {
                    continue;
                };
                simple_cat(
                    BufReader::new(f).by_ref(),
                    &mut stdout,
                    minbufsize(ibufsize, obufsize),
                )?
            }
            Err(e) => {
                // TODO: parse Err
                eprintln!("rat: {}: No such file or directory", &file);
                if strict {
                    return Err(e);
                }
                *ok &= false;
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
