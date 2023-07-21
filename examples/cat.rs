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
        // TODO: determine why write calls are not fully buffered (see strace)
        /*
         * read(0, "\241\371\370\243m\34\0\2738\341%\363\3363o6T\17~\337\34Zt\276\325\311\327\31\351\232\2176"..., 131072) = 131072 
         * write(1, "\241\371\370\243m\34\0\2738\341%\363\3363o6T\17~\337\34Zt\276\325\311\327\31\351\232\2176"..., 131062) = 131062
         * write(1, "\260p\r\347UM\253V\274\210", 10) = 10                                                                          
         * read(0, "\\\255_?\322\37\273)\312\24~'\240b\270&\202\352-\30Kr\215\247wq\233\335\333o\2\327"..., 131072) = 131072        
         * write(1, "\\\255_?\322\37\273)\312\24~'\240b\270&\202\352-\30Kr\215\247wq\233\335\333o\2\327"..., 130904) = 130904       
         * write(1, "<\324\371\0\355\211\272\221\271\34D\267H\16\312\326;\263\265\370\v+\307'\ts\254\264ql)\327"..., 168) = 168     
         */
        let io_bufsize = 65536*2;
        // Vec<u8> holds UTF-8 characters
        let mut buffer: Vec<u8> = vec![];
        let mut reader = BufReader::new(stdin);
        loop {
            // Use `take` to limit reads given the desired bufsize
            // As compared to using `read_exact` which requires a sized vector
            // which can ultimately leave trailing null bytes when reaching EOF
            let mut handle = reader.by_ref().take(io_bufsize);
            match handle.read_to_end(&mut buffer) {
                Ok(0)  => { /* EOF */ break; }
                // Using `.write()` here leads to missing data in output on large streams
                Ok(..) => { stdout.write_all(&buffer)?; buffer.clear() }
                Err(_) => { break; }
            };
        }
        // Does not seem needed?
        stdout.flush()?;
    }
    Ok(())
}
