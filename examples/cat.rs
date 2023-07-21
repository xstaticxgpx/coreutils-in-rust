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


use std::io::{self, Write, BufRead, BufReader};

const NEW_LINE: u8 = 10;

fn main() -> io::Result<()> {
    {
        let stdin = io::stdin().lock();
        let mut stdout = io::stdout().lock();
        /*
        let lines = io::stdin().lines();
        for line in lines {
            // How to determine missing new line here?
            stdout.write(line.unwrap().as_bytes())?;
            // This will add a newline making output mismatched
            // TODO: read_line instead of lines() ?
            stdout.write(b"\n")?;
        }
        */
        let mut buffer = vec![];
        let mut reader = BufReader::new(stdin);
        loop {
            let r = reader.read_until(NEW_LINE, &mut buffer)?;
            if r == 0 {
                eprintln!("EOF");
                break;
            }
            //eprintln!("{:?}", r);
            //eprintln!("{:?}", buffer);
            eprintln!("Buffer Length: {}", buffer.len());
            stdout.write(&buffer)?;
            buffer.clear();
        }
    }
    Ok(())
}
