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
 * --> don't silently ignore missing files like cat (ie. cat /does/not/exist /etc/hosts)
 * --> support opening each file in pager one and a time
 */

use libc::{sysconf, _SC_PAGESIZE};
use std::cmp::min;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write}; //BufRead
use std::os::linux::fs::MetadataExt;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::FromRawFd; //AsRawFd
use std::process::ExitCode;

const IO_BUFSIZE: u64 = 1 << 17; // or 2^17 or 131072 (bytes) or 32 page sizes (4KB usually)
#[allow(dead_code)]
// TODO: for interactive cat use handle.read_until(NEWLINE_CH, &mut buffer)
const NEWLINE_CH: u8 = 10; // 0x0A
const STDIN_FD: i32 = 0;
const STDOUT_FD: i32 = 1;

extern "C" fn get_pagesize() -> u64 {
    unsafe { sysconf(_SC_PAGESIZE) as u64 }
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
    mut input: BufReader<File>,
    output: &mut BufWriter<File>,
    iobufsize: u64,
) -> io::Result<()> {
    let input = input.by_ref();
    // Vec<u8> holds binary UTF-8 characters, no concept of "text mode" here
    let mut buffer: Vec<u8> = Vec::with_capacity(iobufsize as usize);
    // Use `take` and `read_to_end` to limit reads given the desired bufsize
    // As compared to using `read_exact` which requires a sized vector
    // which can ultimately leave trailing null bytes when writing EOF
    #[rustfmt::skip]
    let mut read =  |buffer: &mut Vec<u8>| -> io::Result<usize> {
        input.take(iobufsize).read_to_end(buffer)
    };
    // Use `write_all` here to ensure we aren't dropping any output
    let mut write = |buffer: &mut Vec<u8>| -> io::Result<()> {
        output.write_all(buffer)?;
        buffer.clear();
        Ok(())
    };
    loop {
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
    let (mut _bin, mut _bout) = (IO_BUFSIZE, IO_BUFSIZE);
    let page_size = get_pagesize();
    let iobufsize = |i, o| min(i, o);

    // Not sure these locks are necessary, doesn't hurt?
    let (_, _) = (io::stdin().lock(), io::stdout().lock());
    let mut stdout = BufWriter::new(unsafe { File::from_raw_fd(STDOUT_FD) });
    let mut _stdin = || unsafe { File::from_raw_fd(STDIN_FD) };

    // Determine output characteristics, follows the /dev/stdout symlink
    let _stdout_stat = fs::metadata("/dev/stdout")?;
    if _stdout_stat.file_type().is_fifo() {
        /*
         * Linux pipes are capped at 2^16 == 65536 byte (16 * 4KB pages) max buffer by default:
         * https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/linux/pipe_fs_i.h
         * PAGE_SIZE = 1 << 12; PIPE_DEF_BUFFERS = 16; PAGE_SIZE * PIPE_DEF_BUFFERS == 65536
         */
        _bout = 16 * page_size;
    }

    // Skip the cmdline arg here and re-collect as Strings
    let mut args: Vec<String> = env::args().skip(1).collect();
    // Pass implict stdin if not given any parameters
    if args.len() == 0 {
        args.push(String::from("-"));
    }
    // Only accept positional file arguments for now
    let _stdin_stat = fs::metadata("/dev/stdin")?;
    for arg in args {
        // You could also pass the stdin character devices...
        if arg == "-" || arg == "/dev/stdin" || arg == "/proc/self/fd/0" {
            if _stdin_stat.file_type().is_fifo() {
                _bin = 16 * page_size;
            }
            simple_cat(
                BufReader::new(_stdin()),
                &mut stdout,
                iobufsize(_bin, _bout),
            )?;
            continue;
        }

        let ref file = arg;
        // Use `if let` here to ignore ENOENT error on stat
        if let Ok(_) = fs::metadata(file) {
            if _stdin_stat.is_file()
                && _stdout_stat.is_file()
                && _stdin_stat.st_dev() == _stdout_stat.st_dev()
                && _stdin_stat.st_ino() == _stdout_stat.st_ino()
            {
                /*
                 * Same as coreutils cat. To test:
                 * echo hello > foo
                 * rat foo >> foo
                 *
                 * Note the append (>>), when overwriting (>) the shell completely truncates the file
                 * which results in a empty file regardless of whatever command is run. (ie. `:>foo`)
                 */
                eprintln!("rat: {}: input file is output file", file);
                continue;
            }
        }

        match File::open(file) {
            Ok(f) => {
                simple_cat(BufReader::new(f), &mut stdout, iobufsize(_bin, _bout))?;
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
