"""Font discovery tests for render.py; stdlib only: python3 -m unittest (from scripts/shot)."""
import os
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

try:
    import PIL  # noqa: F401
except ImportError:  # render.py exits without Pillow; font discovery doesn't need it
    pil = types.ModuleType("PIL")
    pil.Image = pil.ImageDraw = pil.ImageFont = types.ModuleType("PIL.stub")
    sys.modules["PIL"] = pil

import render

FONT = "/usr/share/fonts/TTF/DejaVuSansMono.ttf"


def ran(returncode=0, stdout="", stderr=""):
    return subprocess.CompletedProcess([], returncode, stdout, stderr)


class DefaultFont(unittest.TestCase):
    def discover(self, menlo=False, which="/usr/bin/fc-match", run=None):
        with mock.patch("os.path.exists", side_effect=lambda p: menlo and p == render.MENLO), \
             mock.patch("shutil.which", return_value=which) as w, \
             mock.patch("subprocess.run", **({"side_effect": run} if isinstance(run, BaseException) else {"return_value": run})) as r:
            return render.default_font(), w, r

    def exits(self, **kw):
        with self.assertRaises(SystemExit) as e:
            self.discover(**kw)
        self.assertIn("pass --font <path>", str(e.exception.code))
        return str(e.exception.code)

    def test_menlo_wins_when_present(self):
        found, which, run = self.discover(menlo=True, run=ran(stdout=FONT))
        self.assertEqual(found, render.MENLO)
        which.assert_not_called()
        run.assert_not_called()

    def test_fontconfig_result_used_on_linux(self):
        found, _, run = self.discover(run=ran(stdout=FONT + "\n"))
        self.assertEqual(found, FONT)
        args = run.call_args[0][0]
        self.assertEqual(args[0], "/usr/bin/fc-match")
        self.assertIn("monospace:charset=" + render.NEEDED_GLYPHS, args)

    def test_fc_match_missing(self):
        self.assertIn("no fc-match", self.exits(which=None))

    def test_fc_match_fails(self):
        msg = self.exits(run=ran(returncode=1, stderr="Fontconfig error: cannot load config"))
        self.assertIn("fc-match failed", msg)
        self.assertIn("cannot load config", msg)

    def test_fc_match_fails_silently(self):
        self.assertIn("exit status 2", self.exits(run=ran(returncode=2)))

    def test_fc_match_empty(self):
        msg = self.exits(run=ran(stdout="  \n"))
        self.assertIn("found no monospace font", msg)
        self.assertNotIn("no fc-match", msg)

    def test_fc_match_cannot_launch(self):
        self.assertIn("could not run fc-match", self.exits(run=PermissionError(13, "Permission denied")))


class ExplicitFont(unittest.TestCase):
    def test_font_flag_bypasses_discovery(self):
        with tempfile.TemporaryDirectory() as d:
            ansi = os.path.join(d, "in.ansi")
            with open(ansi, "w") as f:
                f.write("hi\n")
            argv = ["render.py", ansi, os.path.join(d, "out.png"), "--font", "/fonts/Mine.ttf"]
            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(render, "default_font", wraps=render.default_font) as discover, \
                 mock.patch("shutil.which") as which, \
                 mock.patch("subprocess.run") as run, \
                 mock.patch.object(render, "render", return_value=(10, 20)) as draw, \
                 mock.patch("builtins.print"):
                render.main()
        self.assertEqual(draw.call_args[0][2], "/fonts/Mine.ttf")
        discover.assert_not_called()
        which.assert_not_called()
        run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
