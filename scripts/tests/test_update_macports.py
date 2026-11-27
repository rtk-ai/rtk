import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    "update_macports", Path(__file__).parents[1] / "update-macports.py"
)
updater = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(updater)

PORTFILE = r'''PortSystem 1.0
PortGroup github 1.0
PortGroup cargo 1.0
github.setup        rtk-ai rtk 1.1.0 v
revision            3
maintainers         @example openmaintainer
license             Apache-2 Restrictive
checksums           ${distname}${extract.suffix} \
                    rmd160 OLD_RMD \
                    sha256 OLD_SHA \
                    size 123

destroot {
    xinstall ${worksrcpath}/engine/LICENSE ${destroot}${prefix}/share/doc/rtk
}

cargo.crates \
    old-crate 0.1.0 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
'''
LOCK = '''version = 4
[[package]]
name = "rtk"
version = "1.2.0"
[[package]]
name = "example"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
'''


class UpdateMacPortsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.directory = Path(self.tmp.name)
        self.portfile = self.directory / "Portfile"
        self.portfile.write_text(PORTFILE)
        self.source = self.directory / "source.tar.gz"
        self.write_source()

    def write_source(self, lock=LOCK, version="1.2.0", include_lock=True):
        members = {"Cargo.toml": f'[package]\nname = "rtk"\nversion = "{version}"\n'}
        if include_lock:
            members["Cargo.lock"] = lock
        with tarfile.open(self.source, "w:gz") as archive:
            for name, content in members.items():
                data = content.encode()
                info = tarfile.TarInfo(f"rtk-1.2.0/{name}")
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))

    def update(self, tag="v1.2.0"):
        return updater.update_portfile(self.portfile, self.source, tag)

    def test_updates_source_and_crates_and_preserves_other_metadata(self):
        self.assertTrue(self.update())
        text = self.portfile.read_text()
        self.assertIn("github.setup        rtk-ai rtk 1.2.0 v", text)
        self.assertIn("revision            0", text)
        self.assertIn("maintainers         @example openmaintainer", text)
        self.assertIn("license             Apache-2 Restrictive", text)
        self.assertIn("xinstall ${worksrcpath}/engine/LICENSE", text)
        data = self.source.read_bytes()
        self.assertIn(hashlib.sha256(data).hexdigest(), text)
        self.assertIn(hashlib.new("ripemd160", data).hexdigest(), text)
        self.assertIn(f"size {len(data)}", text)
        self.assertIn("example", text)
        self.assertIn("b" * 64, text)
        self.assertNotIn("old-crate", text)

    def test_rerun_is_a_noop(self):
        self.update()
        before = self.portfile.read_bytes()
        self.assertFalse(self.update())
        self.assertEqual(before, self.portfile.read_bytes())

    def test_same_release_preserves_macports_revision(self):
        self.update()
        self.portfile.write_text(self.portfile.read_text().replace("revision            0", "revision            2"))
        self.assertFalse(self.update())
        self.assertIn("revision            2", self.portfile.read_text())

    def test_same_release_with_changed_source_is_rejected(self):
        self.update()
        self.write_source(lock=LOCK.replace("2.0.0", "2.0.1"))
        before = self.portfile.read_bytes()
        with self.assertRaises(ValueError):
            self.update()
        self.assertEqual(before, self.portfile.read_bytes())

    def test_rejects_downgrade(self):
        with self.assertRaises(ValueError):
            self.update("v1.0.9")
        self.assertEqual(PORTFILE, self.portfile.read_text())

    def test_rejects_prerelease_and_malformed_tags(self):
        for tag in ["v1.2.0-rc.1", "latest", "v01.2.0", "v1.2.0\n", "v1.2.0;echo bad"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                self.update(tag)

    def test_wrong_source_version_leaves_portfile_untouched(self):
        self.write_source(version="1.1.0")
        with self.assertRaises(ValueError):
            self.update()
        self.assertEqual(PORTFILE, self.portfile.read_text())

    def test_missing_lockfile_is_rejected(self):
        self.write_source(include_lock=False)
        with self.assertRaises(ValueError):
            self.update()
        self.assertEqual(PORTFILE, self.portfile.read_text())

    def test_git_dependency_is_rejected(self):
        self.write_source(lock=LOCK.replace("registry+https://github.com/rust-lang/crates.io-index", "git+https://example.com/crate"))
        with self.assertRaises(ValueError):
            self.update()
        self.assertEqual(PORTFILE, self.portfile.read_text())

    def test_invalid_crate_checksum_is_rejected(self):
        self.write_source(lock=LOCK.replace("b" * 64, "invalid"))
        with self.assertRaises(ValueError):
            self.update()

    def test_tcl_metacharacters_in_crate_name_are_rejected(self):
        self.write_source(lock=LOCK.replace('name = "example"', 'name = "[exec bad]"'))
        with self.assertRaises(ValueError):
            self.update()

    def test_changed_port_layout_fails_without_replacing_metadata(self):
        text = PORTFILE.replace("cargo.crates", "unexpected-layout")
        self.portfile.write_text(text)
        with self.assertRaises(ValueError):
            self.update()
        self.assertEqual(text, self.portfile.read_text())


if __name__ == "__main__":
    unittest.main()
