use std::convert::TryInto;
use std::io::{self, Read, Seek, SeekFrom, Write};

use bstr::BString;
use byteorder::{LE, ReadBytesExt as _, WriteBytesExt as _};

const E_INVALID_HEADER: &str = "Invalid header";
const E_UNSUPPORTED_VERSION: &str = "Unsupported version";

fn advance_magic(magic: &mut u32) -> u32 {
    std::mem::replace(magic, magic.wrapping_mul(7).wrapping_add(3))
}

fn read_as_much_as_possible(mut r: impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut pos = 0;
    while pos < buf.len() {
        match r.read(&mut buf[pos..]) {
            Ok(0) => break,
            Ok(n) => pos += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(pos)
}

fn run_codec(
    buf: &mut [u8],
    mut input: impl Read,
    mut output: impl Write,
    mut size: u32,
    mut magic: u32,
) -> io::Result<()> {
    let limit = buf.len();
    assert!(limit % 4 == 0);
    loop {
        let buf = &mut buf[..limit.min(size as usize)];
        let read = read_as_much_as_possible(&mut input, buf)?;
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
    pub name: BString,
    pub offset: u32,
    pub size: u32,
    pub magic: u32,
}

impl RGSSArchiveEntry {
    pub fn read(&self, buf: &mut [u8], mut r: impl Read + Seek, w: impl Write) -> io::Result<()> {
        r.seek(SeekFrom::Start(self.offset as u64))?;
        run_codec(buf, r, w, self.size, self.magic)?;
        Ok(())
    }

    pub fn write(&self, buf: &mut [u8], mut w: impl Write + Seek, r: impl Read) -> io::Result<()> {
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
    pub fn read_header(&mut self, mut r: impl Read) -> io::Result<()> {
        let mut header = [0; 8];
        r.read_exact(&mut header)?;
        if &header[..7] != b"RGSSAD\0" {
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

    pub fn read_entries(&mut self, r: impl Read + Seek) -> io::Result<()> {
        match self.version {
            1 | 2 => self.read_entries_rgssad(r),
            3 => self.read_entries_rgss3a(r),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                E_UNSUPPORTED_VERSION,
            )),
        }
    }

    fn read_entries_rgssad(&mut self, mut r: impl Read + Seek) -> io::Result<()> {
        let mut magic = 0xdeadcafe;
        while let Ok(first) = r.read_u32::<LE>() {
            let name_len = first ^ advance_magic(&mut magic);
            let mut name = vec![0; name_len as usize];
            r.read_exact(&mut name)?;
            for b in name.iter_mut() {
                *b ^= advance_magic(&mut magic) as u8;
            }
            let size = r.read_u32::<LE>()? ^ advance_magic(&mut magic);
            let offset = r.stream_position()? as u32;
            r.seek(SeekFrom::Current(size as i64))?;
            self.entries.push(RGSSArchiveEntry {
                name: name.into(),
                offset,
                size,
                magic,
            });
        }
        Ok(())
    }

    fn read_entries_rgss3a(&mut self, mut r: impl Read) -> io::Result<()> {
        let magic = r.read_u32::<LE>()?;
        self.magic = magic;
        let xor = magic.wrapping_mul(9).wrapping_add(3);
        loop {
            let offset = r.read_u32::<LE>()? ^ xor;
            if offset == 0 {
                break;
            }
            let size = r.read_u32::<LE>()? ^ xor;
            let magic = r.read_u32::<LE>()? ^ xor;
            let name_len = r.read_u32::<LE>()? ^ xor;
            let mut name = vec![0; name_len as usize];
            r.read_exact(&mut name)?;
            for (i, b) in name.iter_mut().enumerate() {
                *b ^= xor.to_le_bytes()[i % 4];
            }
            self.entries.push(RGSSArchiveEntry {
                name: name.into(),
                offset,
                size,
                magic,
            });
        }
        Ok(())
    }

    pub fn write_header(&self, mut w: impl Write) -> io::Result<()> {
        if !(1..=3).contains(&self.version) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                E_UNSUPPORTED_VERSION,
            ));
        }
        w.write_all(&[b'R', b'G', b'S', b'S', b'A', b'D', b'\0', self.version])?;
        Ok(())
    }

    pub fn write_entries(&mut self, w: impl Write) -> io::Result<()> {
        match self.version {
            1 | 2 => self.write_entries_rgssad(w),
            3 => self.write_entries_rgss3a(w),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                E_UNSUPPORTED_VERSION,
            )),
        }
    }

    fn write_entries_rgssad(&mut self, mut w: impl Write) -> io::Result<()> {
        let mut offset = 8u32;
        let mut magic = 0xdeadcafe;
        for entry in &mut self.entries {
            let name_len = entry.name.len().try_into().unwrap();
            w.write_u32::<LE>(name_len ^ advance_magic(&mut magic))?;
            let mut name = entry.name.clone();
            for b in name.iter_mut() {
                *b ^= advance_magic(&mut magic) as u8;
            }
            w.write_all(&name)?;
            w.write_u32::<LE>(entry.size ^ advance_magic(&mut magic))?;
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

    fn write_entries_rgss3a(&mut self, mut w: impl Write) -> io::Result<()> {
        let mut offset = 16u32;
        for entry in &self.entries {
            let name_len = entry.name.len().try_into().unwrap();
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
        w.write_u32::<LE>(magic)?;
        let xor = magic.wrapping_mul(9).wrapping_add(3);
        for entry in &self.entries {
            w.write_u32::<LE>(entry.offset ^ xor)?;
            w.write_u32::<LE>(entry.size ^ xor)?;
            w.write_u32::<LE>(entry.magic ^ xor)?;
            w.write_u32::<LE>(entry.name.len() as u32 ^ xor)?;
            let mut name = entry.name.clone();
            for (i, b) in name.iter_mut().enumerate() {
                *b ^= xor.to_le_bytes()[i % 4];
            }
            w.write_all(&name)?;
        }
        w.write_u32::<LE>(xor)?;
        Ok(())
    }
}
