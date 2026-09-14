automod::dir!(pub "src/cmds/dotnet");

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;
    use std::time::SystemTime;

    /// Writes a fixture file and pins its mtime to an explicit timestamp so
    /// tests never rely on wall-clock deltas or `thread::sleep` separation.
    pub(crate) fn write_trx_with_mtime(path: &Path, contents: &str, modified: SystemTime) {
        std::fs::write(path, contents).expect("write fixture");
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open fixture for write")
            .set_modified(modified)
            .expect("set fixture mtime");
    }
}
