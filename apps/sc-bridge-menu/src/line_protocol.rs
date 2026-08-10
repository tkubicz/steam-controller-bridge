use std::io::BufRead;

pub fn read_bounded_line(
    reader: &mut impl BufRead,
    maximum_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|error| error.to_string())?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err("line ended before its newline delimiter".to_owned())
            };
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len() + newline + 1 > maximum_bytes {
                return Err("line exceeds its bound".to_owned());
            }
            line.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            return Ok(Some(line));
        }
        if line.len() + available.len() >= maximum_bytes {
            return Err("line exceeds its bound".to_owned());
        }
        line.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn the_bound_counts_the_delimiter_and_partial_eof_is_rejected() {
        assert_eq!(
            read_bounded_line(&mut Cursor::new(b"abc\n"), 4).unwrap(),
            Some(b"abc".to_vec())
        );
        assert!(read_bounded_line(&mut Cursor::new(b"abcd\n"), 4).is_err());
        assert!(read_bounded_line(&mut Cursor::new(b"abc"), 4).is_err());
    }

    #[test]
    fn short_lines_do_not_reserve_the_whole_protocol_bound() {
        let line = read_bounded_line(&mut Cursor::new(b"ok\n"), 64 * 1024)
            .unwrap()
            .unwrap();
        assert_eq!(line, b"ok");
        assert!(line.capacity() < 64 * 1024);
    }
}
