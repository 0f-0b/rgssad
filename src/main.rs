use std::fs;
use std::fs::File;
use std::io::SeekFrom;
use std::io::Seek;
use std::io::Read;
use std::io::Write;
use std::io::Error;
use std::io::ErrorKind;
use std::env;
use std::path::Path;
use std::convert::TryInto;

extern crate regex;
use regex::Regex;

static __VERSION__: &str = "0.1.4";

// Errors
static E_INVALIDHDR: &str = "Input file header mismatch.";
static E_INVALIDVER: &str = "Not supported version.";
static E_INVALIDMGC: &str = "Magic number read failed.";


fn advance_magic(magic: &mut u32) -> u32 {
    let old = *magic;
    *magic = magic.wrapping_mul(7).wrapping_add(3);
    old
}

fn ru32(stream: &mut File, result: &mut u32) -> Result<(), Error> {
    let mut buf = [0; 4];
    stream.read_exact(&mut buf)?;
    *result = u32::from_le_bytes(buf);
    Ok(())
}

fn wu32(stream: &mut File, data: &u32) -> Result<(), Error> {
    let buf = data.to_le_bytes();
    stream.write_all(&buf)
}

/// Calls read until the buffer is full or EOF.
fn read_until_full(stream: &mut File, buf: &mut [u8]) -> Result<usize, Error> {
    let mut nb = 0;
    loop {
        match stream.read(&mut buf[nb..]) {
            Ok(0) => return Ok(nb),
            Ok(n) => nb += n,
            Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

struct EntryData {
    offset: u32,
    magic: u32,
    size: u32,
}


struct Coder {
    buf: Vec<u8>,
}

impl Coder {
    /// Encrypts/decrypts file data from stream_in to stream_out.
    fn copy(
        &mut self,
        stream_in: &mut File,
        stream_out: &mut File,
        data: &EntryData,
    ) -> Result<(), Error> {
        assert!(self.buf.len() % 4 == 0); // needed for alignment

        stream_in.seek(SeekFrom::Start(data.offset as u64))?;

        let mut magic = data.magic;
        let mut size = data.size; // remaining bytes to read
        loop {
            let limit = self.buf.len().min(size as usize);
            let count = read_until_full(stream_in, &mut self.buf[..limit])?;
            if count == 0 { return Ok(()) }
            let buf = &mut self.buf[..count];

            let (prefix, middle, suffix) = unsafe { buf.align_to_mut::<u32>() };
            assert!(prefix.len() == 0); // assume buf is aligned

            for i in 0..middle.len() {
                let mut w = u32::from_le(middle[i]);
                w ^= advance_magic(&mut magic);
                middle[i] = w.to_le();
            }

            for i in 0..suffix.len() {
                suffix[i] ^= (magic >> (i * 8)) as u8;
            }

            size -= count as u32;
            stream_out.write_all(buf)?;
        }
    }
}

struct Entry {
    name: String,
    data: EntryData,
}

struct RGSSArchive {
    magic: u32,
    version: u8,
    entry: Vec<Entry>,
    stream: File,
}

impl RGSSArchive {
    fn create(location: &str, version: u8) -> Result<Self, Error> {
        let mut stream = File::create(location)?;
        if version < 1 || version > 3 {
            return Err(Error::new(ErrorKind::InvalidData, E_INVALIDVER));
        }

        stream.write_all(&[b'R', b'G', b'S', b'S', b'A', b'D', version])?;

        let magic = if version == 3 { 0u32 } else { 0xDEADCAFEu32 };
        let entry = vec![];

        Ok(RGSSArchive { magic, version, entry, stream })
    }

    fn open(location: &str) -> Result<Self, Error> {
        let mut stream = File::open(location)?;

        let mut header = [0u8; 8];
        stream.read_exact(&mut header)?;

        if &header[..6] != b"RGSSAD" {
            return Err(Error::new(ErrorKind::InvalidData, E_INVALIDHDR));
        }

        // Check rgssad file version.
        return match header[7] {
            1|2 => RGSSArchive::open_rgssad(stream, header[7]),
              3 => RGSSArchive::open_rgss3a(stream, header[7]),
              _ => Err(Error::new(ErrorKind::InvalidData, E_INVALIDVER)),
        }
    }

    fn open_rgssad(mut stream: File, version: u8) -> Result<Self, Error> {
        let mut magic = 0xDEADCAFEu32;
        let mut entry = vec![];

        loop {
            let mut name_len: u32 = 0;
            if ru32(&mut stream, &mut name_len).is_err() { break }
            name_len ^= advance_magic(&mut magic);

            let mut name = vec![0u8; name_len as usize];
            stream.read_exact(&mut name)?;
            for i in 0..(name_len as usize) {
                name[i] ^= advance_magic(&mut magic) as u8;
                if name[i] == b'\\' { name[i] = b'/' }
            }
            let name = String::from_utf8(name);
            if name.is_err() { break }
            let name = name.unwrap();

            let mut data = EntryData { size: 0, offset: 0, magic: 0 };
            if ru32(&mut stream, &mut data.size).is_err() { break }
            data.size ^= advance_magic(&mut magic);
            data.offset = stream.seek(SeekFrom::Current(0))? as u32;
            data.magic = magic;

            stream.seek(SeekFrom::Current(data.size as i64))?;
            entry.push(Entry { name, data });
        }

        stream.seek(SeekFrom::Start(0))?;
        Ok(RGSSArchive { magic, version, entry, stream })
    }

    fn open_rgss3a(mut stream: File, version: u8) -> Result<Self, Error> {
        let mut magic = 0u32;
        let mut entry = vec![];

        if ru32(&mut stream, &mut magic).is_err() {
            return Err(Error::new(ErrorKind::InvalidData, E_INVALIDMGC));
        }
        magic = magic.wrapping_mul(9).wrapping_add(3);

        loop {
            let mut offset: u32 = 0;
            let mut size: u32 = 0;
            let mut start_magic: u32 = 0;
            let mut name_len: u32 = 0;

            if ru32(&mut stream, &mut offset).is_err() { break };
            offset ^= magic;

            if offset == 0 { break }

            if ru32(&mut stream, &mut size).is_err() { break }
            size ^= magic;

            if ru32(&mut stream, &mut start_magic).is_err() { break}
            start_magic ^= magic;

            if ru32(&mut stream, &mut name_len).is_err() { break }
            name_len ^= magic;

            let mut name = vec![0u8; name_len as usize];
            stream.read_exact(&mut name)?;
            for i in 0..(name_len as usize) {
                name[i] ^= (magic >> 8*(i%4)) as u8;
                if name[i] == b'\\' { name[i] = b'/' }
            }
            let name = String::from_utf8(name);
            if name.is_err() { break }
            let name = name.unwrap();

            let data = EntryData { size, offset, magic: start_magic };

            entry.push(Entry { name, data });
        }

        stream.seek(SeekFrom::Start(0))?;
        Ok(RGSSArchive { magic, version, entry, stream })
    }

    // fn put_key(&self, key: &str, stream: &mut File) -> Result<Entry, Error> {
    //     match self.version {
    //         1|2 => self.put_key_rgssad(stream),
    //           3 => self.put_key_rgss3a(stream),
    //     }
    // }
}


fn usage() {
    println!("Extract rgssad/rgss2a/rgss3a files.
Commands:
    help
    version
    list        <filename>
    unpack      <filename> <location> [<filter>]");
}

fn list(archive: RGSSArchive) {
    for Entry { name, data } in archive.entry {
        println!("{}: EntryData {{ size: {}, offset: {}, magic: {} }}", name, data.size, data.offset, data.magic);
    }
}

fn pack(src: &str, out: &str, version: u8) {
    fn walkdir(archive: &mut RGSSArchive, d: &Path, r: &Path) {
        for entry in fs::read_dir(&d).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walkdir(archive, &path, r);
            } else {
                let name = path.strip_prefix(r).unwrap().to_str().unwrap();
                let size = fs::metadata(&path).unwrap().len();
                let size: u32 = size.try_into().unwrap();

                archive.entry.push(Entry {
                    name: name.to_string(),
                    data: EntryData {
                        size,
                        offset: 0, // calculated later
                        magic: 0, // calculated later
                    }
                });
            }
        }
    };

    let root = Path::new(src);
    if !root.is_dir() {
        println!("FAILED: source is not a directory."); return
    }

    let mut archive = match RGSSArchive::create(out, version) {
        Ok(x) => x,
        Err(e) => {
            println!("FAILED: unable to create output file. {}", e); return
        }
    };
    // First pass: collect file names and sizes
    walkdir(&mut archive, root, root);
    // Second pass: write file data.
    //archive.write_entries()
}

fn unpack(mut archive: RGSSArchive, dir: &str, filter: &str) {
    fn create(location: String) -> File {
        let path = Path::new(location.as_str());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        File::create(path.to_str().unwrap()).unwrap()
    }

    let entries = archive.entry.iter();
    let filter = match Regex::new(filter) {
        Ok(re) => re,
        Err(_) => {
            println!("FAILED: Invalid regex filter: {}", filter); return
        }
    };

    let mut coder = Coder { buf: vec![0u8; 8192] };

    for Entry { name, data } in entries {
        if !filter.is_match(name) { continue }

        println!("Extracting: {}", name);

        let mut file = create(format!("{}/{}", dir, name));
        if let Err(err) = coder.copy(&mut archive.stream, &mut file, data) {
            println!("FAILED: key save failed, {}", err.to_string()); return
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 { usage(); return }
    match args[1].as_str() {
        "help" => usage(),
        "version" => {
            assert!(args.len() == 2);
            println!("version: {}", __VERSION__);
        },
        "list" => {
            assert!(args.len() == 3);
            let archive = RGSSArchive::open(args[2].as_str());
            if let Err(err) = archive {
                println!("FAILED: file parse failed, {}", err.to_string()); return;
            }
            let archive = archive.unwrap();

            list(archive);
        },
        "unpack" => {
            assert!(args.len() > 3 && args.len() < 6);
            let archive = RGSSArchive::open(args[2].as_str());
            if let Err(err) = archive {
                println!("FAILED: file parse failed, {}", err.to_string()); return;
            }
            let archive = archive.unwrap();
            unpack(archive, args[3].as_str(), if args.len() == 5 { args[4].as_str() } else { ".*" });
        },
        "pack" => {
            assert!(args.len() > 3 && args.len() < 6);
            let mut version = 1u8;
            if args.len() == 5 {
                version = match args[4].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        println!("FAILED: {}", E_INVALIDVER); return
                    }
                }
            };
            pack(args[2].as_str(), args[3].as_str(), version);
        },
        _ => usage(),
    }
}
