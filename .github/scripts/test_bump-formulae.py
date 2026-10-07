from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("bump-formulae.py")


class BumpFormulaeTests(unittest.TestCase):
    def _run_script(self, formula: str) -> tuple[subprocess.CompletedProcess[str], Path]:
        environment = os.environ.copy()
        environment.update(
            {
                "VER": "0.2.0",
                "JJ_GT_DARWIN_ARM64": "darwin-arm64-new-sha",
                "JJ_GT_LINUX_X64": "linux-x64-new-sha",
                "JJ_GT_LINUX_ARM64": "linux-arm64-new-sha",
            }
        )

        temporary_directory = tempfile.TemporaryDirectory()
        self.addCleanup(temporary_directory.cleanup)
        root = Path(temporary_directory.name)
        formula_path = root / "Formula" / "jj-gt.rb"
        formula_path.parent.mkdir()
        formula_path.write_text(formula)
        result = subprocess.run(
            [sys.executable, str(SCRIPT)],
            cwd=root,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
        return result, formula_path

    @staticmethod
    def _formula() -> str:
        return '''class JjGt < Formula
  desc "A minimal jj extension"
  homepage "https://example.com/jj-gt"
  version "0.1.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://example.com/jj-gt-0.1.0-darwin-arm64.tar.gz"
      sha256 "darwin-arm64-old-sha"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://example.com/jj-gt-0.1.0-linux-x64.tar.gz"
      sha256 "linux-x64-old-sha"
    else
      url "https://example.com/jj-gt-0.1.0-linux-arm64.tar.gz"
      sha256 "linux-arm64-old-sha"
    end
  end
end
'''

    def test_rewrites_version_and_all_platform_shas(self) -> None:
        result, formula_path = self._run_script(self._formula())

        self.assertEqual(result.returncode, 0)
        self.assertEqual(
            formula_path.read_text(),
            self._formula()
            .replace('version "0.1.0"', 'version "0.2.0"')
            .replace('darwin-arm64-old-sha', 'darwin-arm64-new-sha')
            .replace('linux-x64-old-sha', 'linux-x64-new-sha')
            .replace('linux-arm64-old-sha', 'linux-arm64-new-sha'),
        )

    def test_version_anchor_drift_fails_without_writing(self) -> None:
        formula = self._formula().replace('version "0.1.0"', "version '0.1.0'")
        result, formula_path = self._run_script(formula)

        self.assertEqual(result.returncode, 1)
        self.assertEqual(formula_path.read_bytes(), formula.encode())

    def test_sha_url_anchor_drift_fails_without_writing(self) -> None:
        formula = self._formula().replace(
            "jj-gt-0.1.0-linux-x64.tar.gz", "jj-gt-0.1.0-linux-x64.zip"
        )
        result, formula_path = self._run_script(formula)

        self.assertEqual(result.returncode, 1)
        self.assertEqual(formula_path.read_bytes(), formula.encode())


if __name__ == "__main__":
    unittest.main()
