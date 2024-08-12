use std::convert::TryInto;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::Path;

use regex::Regex;
use rgssad::{RGSSArchive, RGSSArchiveEntry};
use walkdir::WalkDir;

const USAGE: &str = concat!(
    "Extract rgssad/rgss2a/rgss3a files.\n",
    "Commands:\n",
    "    help\n",
    "    version\n",
    "    list <archive>\n",
    "    unpack <archive> <dir> [<filter>]\n",
    "    pack <dir> <archive> [<version>]\n",
    "    repack <dir> <archive> <template>\n",
);
const VERSION: &str = env!("CARGO_PKG_VERSION");
const E_INVALID_REGEX_FILTER: &str = "Invalid regex filter";
const E_INVALID_VERSION: &str = "Invalid version";

fn ensure_file(path: impl AsRef<Path>) -> io::Result<File> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    File::create(path)
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("help") => {
            print!("{}", USAGE);
        }
        Some("version") => {
            assert!(args.len() <= 2);
            println!("{}", VERSION);
        }
        Some("list") => {
            assert!(args.len() <= 3);
            let archive_path = Path::new(&args[2]);
            let mut archive = RGSSArchive::default();
            {
                let mut file = File::open(archive_path)?;
                archive.read_header(&mut file)?;
                archive.read_entries(&mut file)?;
            }
            for entry in &archive.entries {
                println!(
                    "{}: {{ size: {}, offset: {}, magic: {} }}",
                    entry.name, entry.size, entry.offset, entry.magic
                );
            }
        }
        Some("unpack") => {
            assert!(args.len() <= 5);
            let archive_path = Path::new(&args[2]);
            let dir_path = Path::new(&args[3]);
            let filter = args
                .get(4)
                .map(|s| Regex::new(s))
                .transpose()
                .map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{}: {}", E_INVALID_REGEX_FILTER, e),
                    )
                })?;
            let mut archive = RGSSArchive::default();
            {
                let mut file = File::open(archive_path)?;
                archive.read_header(&mut file)?;
                archive.read_entries(&mut file)?;
                let mut buf = vec![0; 8192];
                for entry in &archive.entries {
                    if matches!(filter, Some(ref re) if !re.is_match(&entry.name)) {
                        continue;
                    }
                    println!("Unpacking {}", entry.name);
                    entry.read(
                        &mut buf,
                        &mut file,
                        &mut ensure_file(dir_path.join(&entry.name))?,
                    )?;
                }
            }
        }
        Some("pack") => {
            assert!(args.len() <= 5);
            let dir_path = Path::new(&args[2]);
            let archive_path = Path::new(&args[3]);
            let version = args.get(4).map(|s| s.parse()).transpose().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{}: {}", E_INVALID_VERSION, e),
                )
            })?;
            let mut archive = RGSSArchive {
                version: version.unwrap_or_else(|| {
                    match archive_path.extension().and_then(OsStr::to_str) {
                        Some("rgss3a") => 3,
                        Some("rgss2a") => 2,
                        _ => 1,
                    }
                }),
                ..RGSSArchive::default()
            };
            for entry in WalkDir::new(dir_path)
                .follow_links(true)
                .sort_by_key(|entry| entry.file_name().to_ascii_uppercase())
            {
                let entry = entry?;
                if entry.file_type().is_file() {
                    archive.entries.push(RGSSArchiveEntry {
                        name: entry
                            .path()
                            .strip_prefix(dir_path)
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .to_owned(),
                        size: entry.metadata()?.len().try_into().unwrap(),
                        offset: 0,
                        magic: 0,
                    });
                }
            }
            {
                let mut file = File::create(archive_path)?;
                archive.write_header(&mut file)?;
                archive.write_entries(&mut file)?;
                let mut buf = vec![0; 8192];
                for entry in &archive.entries {
                    println!("Packing {}", entry.name);
                    entry.write(
                        &mut buf,
                        &mut file,
                        &mut File::open(dir_path.join(&entry.name))?,
                    )?;
                }
            }
        }
        Some("repack") => {
            assert!(args.len() <= 5);
            let dir_path = Path::new(&args[2]);
            let archive_path = Path::new(&args[3]);
            let template_path = Path::new(&args[4]);
            let mut archive = RGSSArchive::default();
            {
                let mut file = File::open(template_path)?;
                archive.read_header(&mut file)?;
                archive.read_entries(&mut file)?;
                for entry in &mut archive.entries {
                    entry.size = fs::metadata(dir_path.join(&entry.name))?
                        .len()
                        .try_into()
                        .unwrap();
                }
            }
            {
                let mut file = File::create(archive_path)?;
                archive.write_header(&mut file)?;
                archive.write_entries(&mut file)?;
                let mut buf = vec![0; 8192];
                for entry in &archive.entries {
                    println!("Packing {}", entry.name);
                    entry.write(
                        &mut buf,
                        &mut file,
                        &mut File::open(dir_path.join(&entry.name))?,
                    )?;
                }
            }
        }
        _ => {
            print!("{}", USAGE);
        }
    }
    Ok(())
}
