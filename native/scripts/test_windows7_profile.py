import tempfile
import unittest
from pathlib import Path
import windows7_profile as profile
class AdapterTests(unittest.TestCase):
    def setup_source(self, root):
        base = root / 'base/rand_util_win.cc'; base.parent.mkdir(parents=True)
        base.write_text('#include <windows.h>\n' + profile.OLD_PRNG)
        boring = root / 'third_party/boringssl/src/crypto/rand/windows.cc'; boring.parent.mkdir(parents=True)
        boring.write_text('#include <windows.h>\n' + profile.OLD_CONDITION + '\n#endif\n' + profile.OLD_CONDITION + '\n#endif\n')
        return base, boring
    def test_adapter_is_idempotent_and_retains_modern_branch(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); base, boring = self.setup_source(root)
            first = profile.apply(root); before = [base.read_bytes(), boring.read_bytes()]
            self.assertEqual(first, profile.apply(root)); self.assertEqual(before, [base.read_bytes(), boring.read_bytes()])
            self.assertIn('BCryptGenRandom', base.read_text()); self.assertIn('return FALSE;', base.read_text())
            self.assertIn('LoadLibraryW(L"bcryptprimitives.dll")', base.read_text())
    def test_unknown_source_rejected_before_any_edit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); base, boring = self.setup_source(root); before = base.read_bytes(); boring.write_text('unknown')
            with self.assertRaises(RuntimeError): profile.apply(root)
            self.assertEqual(before, base.read_bytes())

    def test_mac_x86_adapter_is_explicit_guarded_and_idempotent(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); self.setup_source(root)
            win = root / 'build/win/BUILD.gn'; win.parent.mkdir(parents=True)
            win.write_text('if (is_win) {\n' + profile.OLD_MAC_X86_GUARD + '\n}\n')
            before = win.read_bytes()
            profile.apply(root)
            self.assertEqual(win.read_bytes(), before)
            patches = profile.apply(root, allow_mac_x86=True)
            self.assertEqual(len(patches), 3)
            self.assertIn('mojo_rs_windows7_compat && !is_component_build && !is_asan', win.read_text())
            self.assertIn('host_os != "mac" || target_cpu != "x86"', win.read_text())
            changed = win.read_bytes()
            self.assertEqual(patches, profile.apply(root, allow_mac_x86=True))
            self.assertEqual(changed, win.read_bytes())

    def test_unknown_mac_x86_guard_rejects_before_entropy_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); base, boring = self.setup_source(root)
            before = [base.read_bytes(), boring.read_bytes()]
            win = root / 'build/win/BUILD.gn'; win.parent.mkdir(parents=True); win.write_text('unknown')
            with self.assertRaisesRegex(RuntimeError, 'unsupported Chromium Mac/x86 guard'):
                profile.apply(root, allow_mac_x86=True)
            self.assertEqual(before, [base.read_bytes(), boring.read_bytes()])

    def test_existing_entropy_adapter_migrates_to_span_size_type(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); base, _ = self.setup_source(root)
            profile.apply(root)
            text = base.read_text()
            self.assertIn(profile.NEW_CNG_CHUNK, text)
            base.write_text(text.replace(profile.NEW_CNG_CHUNK, profile.OLD_CNG_CHUNK))
            result = profile.apply(root)
            self.assertNotIn(profile.OLD_CNG_CHUNK, base.read_text())
            self.assertEqual(base.read_text(), text)
            self.assertEqual(result, profile.apply(root))
if __name__ == '__main__': unittest.main()
