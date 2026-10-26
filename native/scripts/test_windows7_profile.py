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
if __name__ == '__main__': unittest.main()
