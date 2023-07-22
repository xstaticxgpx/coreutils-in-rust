/*
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


use std::io::{self, Read, Write};

// TODO: determine buffer size
const IO_BUFSIZE: u64 = 1024*128;

fn simple_cat(mut stdin: std::io::StdinLock<>, mut stdout: std::io::StdoutLock<'_>) -> io::Result<()> {
    // TODO: determine why write calls are not fully buffered with /dev/random (see strace)
    // Using /dev/zero results in fully buffered writes (reads are always fully buffered?)
    /*
     * strace ./target/release/examples/cat <t/test.rand >t/test.rand2
     * read(0, "\241\371\370\243m\34\0\2738\341%\363\3363o6T\17~\337\34Zt\276\325\311\327\31\351\232\2176"..., 131072) = 131072
     * write(1, "\241\371\370\243m\34\0\2738\341%\363\3363o6T\17~\337\34Zt\276\325\311\327\31\351\232\2176"..., 131062) = 131062
     * write(1, "\260p\r\347UM\253V\274\210", 10) = 10
     * ###
     * write(1, "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"..., 131072) = 131072
     * read(0, "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"..., 131072) = 131072
     */
    let handle = stdin.by_ref();
    let mut buffer: Vec<u8> = vec![]; // Vec<u8> holds UTF-8 characters
    loop {
        // Use `take` and `read_to_end` to limit reads given the desired bufsize
        // As compared to using `read_exact` which requires a sized vector
        // which can ultimately leave trailing null bytes when reaching EOF
        match handle.take(IO_BUFSIZE).read_to_end(&mut buffer) {
            Ok(0)  => { /* EOF */ break; }
            // Using `.write_all()` here to ensure we aren't missing data in output on large streams
            // Even doing `write()` and `flush()` here doesn't work and drops data randomly (?)
            Ok(..) => { stdout.write_all(&buffer)?; buffer.clear(); }
            Err(_) => { break; }
        };
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let stdin = io::stdin().lock();
    let stdout = io::stdout().lock();
    // TODO: this doesn't work interactively when newline is submitted (needs Ctrl+D / EOF)
    simple_cat(stdin, stdout)
}
