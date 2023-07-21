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
 * ---> `simple_cat` if no output formatting options but irregular files (stdin/stdout)?
 * ---> `cat` if output is formatted
 * --> chain results using bitwise AND operator (&=) on boolean (`ok`) to determine final exit code
 * --> any failures results in non-zero exit code regardless of where it occurred in the loop
 * -> continue loop if more files (arguments) remain
 */


use std::io::{self, Read, Write, BufReader};

fn main() -> io::Result<()> {
    {
        let stdin = io::stdin().lock();
        let mut stdout = io::stdout().lock();
        // TODO: determine buffer size
        let io_bufsize = 32768;
        let mut buffer = vec![];
        let mut reader = BufReader::new(stdin);
        loop {
            // Use `take` to limit reads given the desired bufsize
            // As compared to using `read_exact` which requires a sized vector
            // which can ultimately leave null bytes when reading tail of file
            let mut handle = reader.by_ref().take(io_bufsize);
            match handle.read_to_end(&mut buffer) {
                Ok(0)  => { break; }
                Ok(_)  => { stdout.write(&buffer)?; buffer = vec![] }
                Err(_) => { break; }
            };
        }
    }
    Ok(())
}
