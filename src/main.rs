use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use which::which;

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Clone, Serialize, Deserialize)]
struct Entry {
    directory: String,
    file: String,
    arguments: Vec<String>,
}

fn lock(file: &mut File) -> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::lockf(file.as_raw_fd(), libc::F_LOCK, 0) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK};
        unsafe {
            let mut overlapped = std::mem::zeroed();
            let ret = LockFileEx(
                HANDLE(file.as_raw_handle() as isize),
                LOCKFILE_EXCLUSIVE_LOCK,
                0,
                !0,
                !0,
                &mut overlapped,
            );
            if ret.0 == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            return Ok(());
        };
    }

    #[cfg(not(any(unix, windows)))]
    compile_error!("File (un)locking only supported on Unix and Windows");
}

#[cfg(windows)]
fn unlock(file: &mut File) -> Result<(), Box<dyn Error>> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::UnlockFile;
    let ret = unsafe { UnlockFile(HANDLE(file.as_raw_handle() as isize), 0, 0, !0, !0) };
    if ret.0 == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn process_compile_commands_json(
    json_path: &Path,
    directory: &Path,
    arguments: &[String],
    files: &BTreeSet<String>,
) -> Result<(), Box<dyn Error>> {
    if let Err(error) = File::options()
        .write(true)
        .create_new(true)
        .open(&json_path)
    {
        match error.kind() {
            ErrorKind::AlreadyExists => (),
            _ => return Err(error.into()),
        }
    }

    let directory = directory
        .to_path_buf()
        .into_os_string()
        .into_string()
        .unwrap();

    let mut json_file = File::options().read(true).write(true).open(json_path)?;
    lock(&mut json_file)?;

    let mut data = String::new();
    json_file.read_to_string(&mut data)?;

    let old_entries: BTreeSet<Entry> = if data.trim().is_empty() {
        BTreeSet::new()
    } else {
        serde_json::from_str(&data)?
    };
    let mut new_entries: BTreeSet<Entry> = old_entries
        .iter()
        .filter(|&e| e.directory != directory || !files.contains(&e.file))
        .cloned()
        .collect();
    for f in files {
        new_entries.insert(Entry {
            directory: directory.clone(),
            file: f.to_string(),
            arguments: arguments.to_owned(),
        });
    }

    if new_entries != old_entries {
        let json_string = serde_json::to_string_pretty(&new_entries)?;
        json_file.set_len(0)?;
        json_file.seek(SeekFrom::Start(0))?;
        writeln!(&mut json_file, "{}", json_string)?;
    }

    // On Unix there is no need to explicitly release the lock since this is done implicitly once
    // the file is closed.  On Windows this is more or less the same except that the time between
    // closing the file and releasing the lock may be arbitrarily long.  Thus it is suggested to
    // explicitly unlock the file.
    #[cfg(windows)]
    unlock(&mut json_file)?;

    Ok(())
}

fn find_compiler(cmd: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let file_name = cmd.file_name().unwrap();
    let file_name_str = file_name.to_os_string().into_string().unwrap();

    if let Some(compiler) = file_name_str.strip_prefix("cdbgen-") {
        Ok(which(compiler)?)
    } else {
        Err(format!("command '{}' misses prefix 'cdbgen-'", file_name_str).into())
    }
}

fn exec(compiler: &Path) -> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(Command::new(compiler)
            .args(env::args_os().skip(1))
            .exec()
            .into())
    }

    #[cfg(not(unix))]
    {
        let status = Command::new(compiler)
            .args(env::args_os().skip(1))
            .status()
            .expect("failed to execute process");
        if status.success() {
            Ok(())
        } else {
            if let Some(ecode) = status.code() {
                std::process::exit(ecode)
            }
            Err(format!("{}", status).into())
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args: Vec<_> = env::args().collect();

    let compiler = find_compiler(Path::new(&args[0]))?;

    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let files: BTreeSet<_> = args[1..]
        .iter()
        .filter(|arg| {
            #[cfg(not(windows))]
            let x = arg;
            #[cfg(windows)]
            let x = arg.to_lowercase();
            x.ends_with(".c") || x.ends_with(".cc") || x.ends_with(".cpp")
        })
        .cloned()
        .collect();
    if !files.is_empty() {
        let json_path = env::var_os("CDBGEN").unwrap_or_else(|| "compile_commands.json".into());
        let json_path = Path::new(&json_path);

        let directory = env::current_dir()?;

        args[0] = compiler.to_str().unwrap().to_string();

        process_compile_commands_json(json_path, &directory, &args, &files)?;
    }

    exec(&compiler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_cmd::prelude::*;
    use assert_fs::prelude::*;
    use std::ffi::OsString;
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    #[test]
    fn find_compiler() {
        let bindir0 = assert_fs::TempDir::new().unwrap();

        let bindir1 = assert_fs::TempDir::new().unwrap();
        let foobar1 = bindir1.join("foobar");
        OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o700)
            .open(&foobar1)
            .unwrap();

        let bindir2 = assert_fs::TempDir::new().unwrap();
        let foobar2 = bindir2.join("foobar");
        OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o700)
            .open(&foobar2)
            .unwrap();

        let bindir3 = assert_fs::TempDir::new().unwrap();
        let foobar3 = bindir3.join("foobar");
        OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o700)
            .open(&foobar3)
            .unwrap();

        let paths = [
            bindir0.path(),
            bindir1.path(),
            bindir2.path(),
            bindir3.path(),
        ];
        let old_path = env::var_os("PATH").unwrap_or_else(OsString::new);
        let new_path = env::join_paths(paths.iter()).unwrap();
        env::set_var("PATH", &new_path);

        assert_eq!(
            foobar1,
            super::find_compiler(Path::new("cdbgen-foobar")).unwrap()
        );

        env::set_var("PATH", &old_path);
    }

    #[test]
    fn main() {
        let cmd = Command::cargo_bin("cdbgen").unwrap();
        let cdbgen_path = Path::new(cmd.get_program()).canonicalize().unwrap();
        let temp = assert_fs::TempDir::new().unwrap();
        temp.child("cdbgen-true")
            .symlink_to_file(cdbgen_path)
            .unwrap();

        let path = format!("{}:/bin:/usr/bin", temp.path().display());

        let n = 100;

        let handles: Vec<_> = (0..n)
            .map(|i| {
                Command::new("cdbgen-true")
                    .args([
                        "-O2",
                        "-o",
                        &format!("foo{:03}", i),
                        &format!("foo{:03}.c", i),
                    ])
                    .env("PATH", &path)
                    .current_dir(temp.path())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut h in handles {
            h.wait().unwrap();
        }
        // do the same again
        let handles: Vec<_> = (0..n)
            .map(|i| {
                Command::new("cdbgen-true")
                    .args([
                        "-O2",
                        "-o",
                        &format!("foo{:03}", i),
                        &format!("foo{:03}.c", i),
                    ])
                    .env("PATH", &path)
                    .current_dir(temp.path())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut h in handles {
            h.wait().unwrap();
        }

        let json_file_path = temp.path().join("compile_commands.json");
        let mut json_file = File::options().read(true).open(json_file_path).unwrap();
        let mut data = String::new();
        json_file.read_to_string(&mut data).unwrap();
        let mut entries: Vec<Entry> = serde_json::from_str(&data).unwrap();
        entries.sort();

        assert_eq!(entries.len(), n);

        for i in 0..n {
            assert_eq!(entries[i].directory, temp.path().to_string_lossy());
            assert_eq!(entries[i].file, format!("foo{:03}.c", i));
            let args = [
                "/bin/true",
                "-O2",
                "-o",
                &format!("foo{:03}", i),
                &format!("foo{:03}.c", i),
            ];
            assert_eq!(entries[i].arguments, args);
        }
    }

    #[test]
    fn mutiple_compilation_units() {
        let cmd = Command::cargo_bin("cdbgen").unwrap();
        let cdbgen_path = Path::new(cmd.get_program()).canonicalize().unwrap();
        let temp = assert_fs::TempDir::new().unwrap();
        temp.child("cdbgen-true")
            .symlink_to_file(cdbgen_path)
            .unwrap();

        let path = format!("{}:/bin:/usr/bin", temp.path().display());

        let status = Command::new("cdbgen-true")
            .args(["-O2", "baz.c", "-o", "foo.c", "bar.c"])
            .env("PATH", &path)
            .current_dir(temp.path())
            .status()
            .unwrap();
        assert!(status.success());

        let json_file_path = temp.path().join("compile_commands.json");
        let mut json_file = File::options().read(true).open(json_file_path).unwrap();
        let mut data = String::new();
        json_file.read_to_string(&mut data).unwrap();
        let mut entries: Vec<Entry> = serde_json::from_str(&data).unwrap();
        entries.sort();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].file, "bar.c");
        assert_eq!(entries[1].file, "baz.c");
        assert_eq!(entries[2].file, "foo.c");
    }
}
