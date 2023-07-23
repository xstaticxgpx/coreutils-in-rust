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
 * ---> `copy_cat` if no output formatting options provided and input/output are regular files
 * ----> if error use `simple_cat`
 * ---> `simple_cat` if no output formatting options but irregular files (stdin/stdout)?
 * ---> `cat` if output is formatted
 * --> chain results using bitwise AND operator (&=) on boolean (`ok`) to determine final exit code
 * --> any failures results in non-zero exit code regardless of where it occurred in the loop
 * -> continue loop if more files (arguments) remain
 */


#[allow(unused_imports)]
use std::fs::File;
use std::io::{self, Read, Write, BufRead, BufWriter};
use std::os::unix::io::FromRawFd;

#[allow(dead_code)]
// TODO: get actual page size?
const IO_BUFSIZE: u64 = 1 << 17; // or 2^17 or 131072 (bytes) or 32 page sizes (4Kb usually)
#[allow(dead_code)]
const NEWLINE_CH: u8  = 10; // 0x0A

fn simple_cat(mut stdin: std::io::StdinLock<>, mut stdout: BufWriter<File>) -> io::Result<()> {
    // TODO: determine why write calls are not fully buffered with /dev/random (see strace)
    // Interesting... when piping input vs stdin file redirection the buffer sizes are different?
    // Yes - linux kernel caps at 65536 byte (16 * 4Kb pages) max buffer across pipes by default (minimum 8192):
    // https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/linux/pipe_fs_i.h
    // PAGE_SIZE = 1 << 12; PIPE_DEF_BUFFERS = 16; PAGE_SIZE * PIPE_DEF_BUFFERS == 65536
    // PAGE_SIZE = 1 << 12; PIPE_MIN_DEF_BUFFERS = 2; PAGE_SIZE * PIPE_MIN_DEF_BUFFERS == 8192
    /*
     * strace ./target/release/examples/cat <t/test.rand >t/test.rand2
     * read(0, "\241\371\370\243m\34\0\2738\341%\363\3363o6T\17~\337\34Zt\276\325\311\327\31\351\232\2176"..., 131072) = 131072
     * write(1, "\241\371\370\243m\34\0\2738\341%\363\3363o6T\17~\337\34Zt\276\325\311\327\31\351\232\2176"..., 131062) = 131062
     * write(1, "\260p\r\347UM\253V\274\210", 10) = 10
     * ###
     * write(1, "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"..., 131072) = 131072
     * read(0, "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"..., 131072) = 131072
     *
     * It's flushing writes on newline characters ??? (`strace -x -s $((2**17))`):
     * write(1, ".....garbage.....\x0a", 40808) = 40808
     * write(1, ".....garbage.........", , 152) = 152
     *
     * Oh my... StdoutLock wraps LineWriter:
     * https://doc.rust-lang.org/std/io/struct.LineWriter.html
     * https://github.com/rust-lang/rust/blob/8771282d4e7a5c4569e49d1f878fb3ba90a974d0/library/std/src/io/stdio.rs#L535
     * https://github.com/rust-lang/rust/blob/8771282d4e7a5c4569e49d1f878fb3ba90a974d0/library/std/src/io/buffered/linewriter.rs
     */
    let handle = stdin.by_ref();
    // HACK!
    // Padding the buffer here but immediately clear so we can avoid `mremap` during runtime
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
        stdout.flush()?;
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let stdin = io::stdin().lock();
    // TODO: this doesn't work interactively when newline is submitted (needs Ctrl+D / EOF)
    // This works - results in 8192 read buffer sizes and seemingly arbitrary writes (on newlines?)
    //match handle.read_until(NEWLINE_CH, &mut buffer) {
    unsafe { 
        // Stdout is wrapped my LineWriter which flushes when newline characters, instead
        // of performing fully buffered writes like expected. So, interface directly with the raw FD.
        let stdout = BufWriter::new(File::from_raw_fd(1)); //BufWriter::new(io::stdout().lock());
        simple_cat(stdin, stdout)
    }
}
