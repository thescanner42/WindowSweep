use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
};

use anyreader_walker::{AnyWalker, EntryDetails, FileEntry, FormatKind};
use walkdir::WalkDir;

pub struct ScanElement<'a> {
    /// a pseudo-path like representation which indicates where something was
    /// found. if inside an expanded archive, the path element is prefixed by !.
    ///
    /// this path might not exist - should not be used to open files directly
    pub pseudo_path: &'a Path,
    pub entry: &'a mut dyn std::io::Read,
}

/// recursive walk starting from root, recursively walks inside archive files
///
/// exclude accepts the pseudo-path (as described above) and if true is given
/// then it will not scan nor traverse that folder, nor will it expand that part
/// of the archive
pub fn walk(
    root: &Path,
    exclude: impl Fn(&Path) -> bool + Sync,
    archive_recursion_max_depth: usize,
    handler: impl Fn(ScanElement) -> () + Sync,
) -> () {
    struct Visitor<'a, Exclude: Fn(&Path) -> bool, H: Fn(ScanElement) -> ()> {
        archive_stack: Vec<PathBuf>,
        handler: &'a H,
        exclude: &'a Exclude,
        archive_recursion_max_depth: usize,
    }

    impl<'a, Exclude: Fn(&Path) -> bool, H: Fn(ScanElement) -> ()> AnyWalker
        for Visitor<'a, Exclude, H>
    {
        fn visit_file_entry(&mut self, entry: &mut FileEntry<impl Read>) -> std::io::Result<()> {
            let pseudo_path = self
                .archive_stack
                .last()
                .unwrap_or(&Default::default())
                .join(entry.path());

            if (self.exclude)(&pseudo_path) {
                return Ok(());
            }

            let arg = ScanElement { pseudo_path: &pseudo_path, entry };

            (self.handler)(arg);
            Ok(())
        }

        fn begin_visit_archive(
            &mut self,
            details: &EntryDetails,
            _format: FormatKind,
        ) -> std::io::Result<bool> {
            if self.archive_stack.len() == self.archive_recursion_max_depth {
                return Ok(false);
            }

            let mut new = self.archive_stack.last().cloned().unwrap_or_default();

            if let Some(name) = details.path.file_name() {
                if let Some(parent) = details.path.parent() {
                    new.push(parent);
                }

                let mut marked = OsString::from("!");
                marked.push(name);
                new.push(marked);
            } else {
                new.push(details.path.as_path());
            }

            if (self.exclude)(&new) {
                return Ok(false);
            }

            self.archive_stack.push(new);
            Ok(true)
        }

        fn end_visit_archive(
            &mut self,
            _details: EntryDetails,
            _format: FormatKind,
        ) -> std::io::Result<()> {
            self.archive_stack.pop();
            Ok(())
        }
    }

    rayon::scope(|s| {
        for entry in WalkDir::new(root).into_iter().filter_map(|e| {
            let e = e.ok()?;
            if exclude(e.path()) {
                return None;
            }
            Some(e)
        }) {
            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path().to_owned();
            let entry = match FileEntry::from_path(path) {
                Ok(v) => v,
                Err(_) => {
                    // e.g. couldn't fstat the file for it's info?
                    // skip this error case
                    continue;
                },
            };
            s.spawn(|_| {
                Visitor {
                    archive_stack: Default::default(),
                    handler: &handler,
                    exclude: &exclude,
                    archive_recursion_max_depth,
                }
                .walk(entry)
                .unwrap(); // unwrap ok since defined begin_visit_archive and end_visit_archive above never return Err
            })
        }
    });
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn tester() {
//         walk(PathBuf::from("test_dir").as_path(), |pseudo_path| {
//             pseudo_path.as_os_str().to_string_lossy().contains("!")
//             // true
//          }, 100, |a| {
//             println!("{:?}", a.pseudo_path);
//             Ok(())
//         }).unwrap();
//     }
// }
