mod io_util;

use std::convert::TryInto;
use std::io::{self, Read, Seek, SeekFrom, Write};

use io_util::{ReadFull, ReadNum, WriteNum};

const E_INVALID_HEADER: &str = "Invalid header";
const E_UNSUPPORTED_VERSION: &str = "Unsupported version";

fn advance_magic(magic: &mut u32) -> u32 {
    std::mem::replace(magic, magic.wrapping_mul(7).wrapping_add(3))
}

fn run_codec(
    buf: &mut [u8],
    input: &mut impl Read,
    output: &mut impl Write,
    mut size: u32,
    mut magic: u32,
) -> io::Result<()> {
    let limit = buf.len();
    assert!(limit % 4 == 0);
    loop {
        let buf = &mut buf[..limit.min(size as usize)];
        let read = input.read_full(buf)?;
        if read == 0 {
            break;
        }
        let buf = &mut buf[..read];
        let (prefix, middle, suffix) = unsafe { buf.align_to_mut::<u32>() };
        assert!(prefix.is_empty());
        for b in middle.iter_mut() {
            *b ^= advance_magic(&mut magic).to_le();
        }
        for (i, b) in suffix.iter_mut().enumerate() {
            *b ^= magic.to_le_bytes()[i];
        }
        size -= read as u32;
        output.write_all(buf)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RGSSArchiveEntry {
    pub name: String,
    pub size: u32,
    pub offset: u32,
    pub magic: u32,
}

impl RGSSArchiveEntry {
    pub fn read(
        &self,
        buf: &mut [u8],
        r: &mut (impl Read + Seek),
        w: &mut impl Write,
    ) -> io::Result<()> {
        r.seek(SeekFrom::Start(self.offset as u64))?;
        run_codec(buf, r, w, self.size, self.magic)?;
        Ok(())
    }

    pub fn write(
        &self,
        buf: &mut [u8],
        w: &mut (impl Write + Seek),
        r: &mut impl Read,
    ) -> io::Result<()> {
        w.seek(SeekFrom::Start(self.offset as u64))?;
        run_codec(buf, r, w, self.size, self.magic)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RGSSArchive {
    pub version: u8,
    pub entries: Vec<RGSSArchiveEntry>,
    pub magic: u32,
}

impl RGSSArchive {
    pub fn read_header(&mut self, r: &mut impl Read) -> io::Result<()> {
        let mut header = [0; 8];
        r.read_exact(&mut header)?;
        if &header[..6] != b"RGSSAD" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, E_INVALID_HEADER));
        }
        self.version = header[7];
        if !(1..=3).contains(&self.version) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                E_UNSUPPORTED_VERSION,
            ));
        }
        Ok(())
    }

    pub fn read_entries(&mut self, r: &mut (impl Read + Seek)) -> io::Result<()> {
        match self.version {
            1 | 2 => self.read_entries_rgssad(r),
            3 => self.read_entries_rgss3a(r),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                E_UNSUPPORTED_VERSION,
            )),
        }
    }

    fn read_entries_rgssad(&mut self, r: &mut (impl Read + Seek)) -> io::Result<()> {
        let mut magic = 0xdeadcafe;
        loop {
            let mut name = vec![
                0;
                match r.read_u32_le() {
                    Ok(x) => x ^ advance_magic(&mut magic),
                    Err(_) => break,
                } as usize
            ];
            r.read_exact(&mut name)?;
            for b in name.iter_mut() {
                *b ^= advance_magic(&mut magic) as u8;
            }
            for b in name.iter_mut() {
                if *b == b'\\' {
                    *b = b'/';
                }
            }
            let name = match String::from_utf8(name) {
                Ok(x) => x,
                Err(_) => break,
            };
            let size = match r.read_u32_le() {
                Ok(x) => x ^ advance_magic(&mut magic),
                Err(_) => break,
            };
            let offset = r.stream_position()? as u32;
            r.seek(SeekFrom::Current(size as i64))?;
            self.entries.push(RGSSArchiveEntry {
                name,
                size,
                offset,
                magic,
            });
        }
        Ok(())
    }

    fn read_entries_rgss3a(&mut self, r: &mut impl Read) -> io::Result<()> {
        let magic = r.read_u32_le()?;
        self.magic = magic;
        let xor = magic.wrapping_mul(9).wrapping_add(3);
        loop {
            let offset: u32 = match r.read_u32_le() {
                Ok(x) => x ^ xor,
                Err(_) => break,
            };
            if offset == 0 {
                break;
            }
            let size: u32 = match r.read_u32_le() {
                Ok(x) => x ^ xor,
                Err(_) => break,
            };
            let magic: u32 = match r.read_u32_le() {
                Ok(x) => x ^ xor,
                Err(_) => break,
            };
            let mut name = vec![
                0;
                match r.read_u32_le() {
                    Ok(x) => x ^ xor,
                    Err(_) => break,
                } as usize
            ];
            r.read_exact(&mut name)?;
            for (i, b) in name.iter_mut().enumerate() {
                *b ^= xor.to_le_bytes()[i % 4];
            }
            for b in name.iter_mut() {
                if *b == b'\\' {
                    *b = b'/';
                }
            }
            let name = match String::from_utf8(name) {
                Ok(x) => x,
                Err(_) => break,
            };
            self.entries.push(RGSSArchiveEntry {
                name,
                size,
                offset,
                magic,
            });
        }
        Ok(())
    }

    pub fn write_header(&self, w: &mut impl Write) -> io::Result<()> {
        if !(1..=3).contains(&self.version) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                E_UNSUPPORTED_VERSION,
            ));
        }
        w.write_all(&[b'R', b'G', b'S', b'S', b'A', b'D', b'\0', self.version])?;
        Ok(())
    }

    pub fn write_entries(&mut self, w: &mut impl Write) -> io::Result<()> {
        match self.version {
            1 | 2 => self.write_entries_rgssad(w),
            3 => self.write_entries_rgss3a(w),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                E_UNSUPPORTED_VERSION,
            )),
        }
    }

    fn write_entries_rgssad(&mut self, w: &mut impl Write) -> io::Result<()> {
        let mut offset = 8u32;
        let mut magic = 0xdeadcafe;
        for entry in &mut self.entries {
            let name_len: u32 = entry.name.len().try_into().unwrap();
            w.write_u32_le(name_len ^ advance_magic(&mut magic))?;
            let mut name = entry.name.as_bytes().to_owned();
            for b in name.iter_mut() {
                if *b == b'/' {
                    *b = b'\\';
                }
            }
            for b in name.iter_mut() {
                *b ^= advance_magic(&mut magic) as u8;
            }
            w.write_all(&name)?;
            w.write_u32_le(entry.size ^ advance_magic(&mut magic))?;
            offset = offset
                .checked_add(name_len)
                .unwrap()
                .checked_add(8)
                .unwrap();
            entry.offset = offset;
            entry.magic = magic;
            w.write_all(&vec![0; entry.size as usize])?;
            offset = offset.checked_add(entry.size).unwrap();
        }
        Ok(())
    }

    fn write_entries_rgss3a(&mut self, w: &mut impl Write) -> io::Result<()> {
        let mut offset: u32 = 16u32;
        for entry in &self.entries {
            let name_len: u32 = entry.name.len().try_into().unwrap();
            offset = offset
                .checked_add(name_len)
                .unwrap()
                .checked_add(16)
                .unwrap();
        }
        for entry in &mut self.entries {
            entry.offset = offset;
            offset = offset.checked_add(entry.size).unwrap();
        }
        let magic = self.magic;
        w.write_u32_le(magic)?;
        let xor = magic.wrapping_mul(9).wrapping_add(3);
        for entry in &self.entries {
            w.write_u32_le(entry.offset ^ xor)?;
            w.write_u32_le(entry.size ^ xor)?;
            w.write_u32_le(entry.magic ^ xor)?;
            w.write_u32_le(entry.name.len() as u32 ^ xor)?;
            let mut name = entry.name.as_bytes().to_owned();
            for b in name.iter_mut() {
                if *b == b'/' {
                    *b = b'\\';
                }
            }
            for (i, b) in name.iter_mut().enumerate() {
                *b ^= xor.to_le_bytes()[i % 4];
            }
            w.write_all(&name)?;
        }
        w.write_u32_le(xor)?;
        Ok(())
    }
}
