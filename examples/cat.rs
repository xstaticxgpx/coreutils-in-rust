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
 * --> FADVISE_SEQUENTIAL if regular file
 * --> error if _non empty_ input file (regular) == output file, continue
 * --> determine implementation (copy_cat / simple_cat / cat)
 * ---> `copy_cat` if no output formatting options provided and input/output are regular files both on the same device (?)
 * ---> `simple_cat` if no output formatting options and `copy_cat` fails because irregular files (stdin/stdout) or different devices (?)
 * ---> `cat` if output is formatted
 * --> chain results using bitwise AND operator (&=) on boolean (`ok`) to determine final exit code
 * --> any failures results in non-zero exit code regardless of where it occurred in the loop
 * -> continue loop if more files (arguments) remain
 */


use std::fs::File;
#[allow(unused_imports)]
use std::io::{self, Read, Write, BufRead, BufWriter};
use std::os::unix::io::FromRawFd;

// Linux pipes are capped at 65536 byte (16 * 4Kb pages) max buffer by default:
// https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/linux/pipe_fs_i.h
// PAGE_SIZE = 1 << 12; PIPE_DEF_BUFFERS = 16; PAGE_SIZE * PIPE_DEF_BUFFERS == 65536
// No such limit seems to exist for regular file descriptors so IO_BUFSIZE has no ceiling
#[allow(dead_code)]
// TODO: get actual page size?
const IO_BUFSIZE: u64 = 1 << 17; // or 2^17 or 131072 (bytes) or 32 page sizes (4Kb usually)
#[allow(dead_code)]
// TODO: for interactive cat use handle.read_until(NEWLINE_CH, &mut buffer)
const NEWLINE_CH: u8  = 10; // 0x0A

fn simple_cat(mut stdin: std::io::StdinLock<>, mut stdout: BufWriter<File>) -> io::Result<()> {
    let handle = stdin.by_ref();
    // HACK!
    // Padding the buffer here then immediately clear prevents subsequent `mremap` during runtime
    // This also fixes the strange read ramp up seen (8192 .. 8192 .. 16384 .. etc .. IO_BUFSIZE)
    // compared to when an unsized, empty vector is specified
    let mut buffer: Vec<u8> = vec![0; IO_BUFSIZE as usize]; // Vec<u8> holds UTF-8 characters
    buffer.clear();
    loop {
        // Use `take` and `read_to_end` to limit reads given the desired bufsize
        // As compared to using `read_exact` which requires a sized vector
        // which can ultimately leave trailing null bytes when writing EOF
        match handle.take(IO_BUFSIZE).read_to_end(&mut buffer) {
            Ok(0)  => { /* EOF */ break; }
            // Use `write_all` here to ensure we aren't dropping any output
            Ok(..) => { stdout.write_all(&buffer)?; buffer.clear(); }
            Err(_) => { break; }
        };
    }
    stdout.flush()
}

fn main() -> io::Result<()> {
    /*
     * Stdout/StdoutLock is wrapped by LineWriter which always flushes writes on newline char:
     * https://doc.rust-lang.org/std/io/struct.LineWriter.html
     * https://github.com/rust-lang/rust/blob/8771282d4e7a5c4569e49d1f878fb3ba90a974d0/library/std/src/io/stdio.rs#L535
     * https://github.com/rust-lang/rust/blob/8771282d4e7a5c4569e49d1f878fb3ba90a974d0/library/std/src/io/buffered/linewriter.rs
     *
     * Instead, use BufWriter directly on the raw file desc (std::io::StdoutRaw is not exposed)
     * This seems like a common complaint:
     * https://github.com/rust-lang/rust/issues/58326
     * https://github.com/rust-lang/libs-team/issues/148
     */
    let stdin = io::stdin().lock();
    let stdout = BufWriter::new(unsafe { File::from_raw_fd(1) });
    simple_cat(stdin, stdout)
}
