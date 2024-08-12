use std::io::{self, Read, Write};

pub trait ReadFull {
    fn read_full(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

impl<R: Read> ReadFull for R {
    fn read_full(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut pos = 0;
        loop {
            match self.read(&mut buf[pos..]) {
                Ok(0) => return Ok(pos),
                Ok(n) => pos += n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

pub trait ReadNum {
    fn read_u32_le(&mut self) -> io::Result<u32>;
}

impl<R: Read> ReadNum for R {
    fn read_u32_le(&mut self) -> io::Result<u32> {
        let mut buf = [0; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }
}

pub trait WriteNum {
    fn write_u32_le(&mut self, value: u32) -> io::Result<()>;
}

impl<W: Write> WriteNum for W {
    fn write_u32_le(&mut self, value: u32) -> io::Result<()> {
        self.write_all(&value.to_le_bytes())
    }
}
